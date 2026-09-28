/*!
 * io_uring Network Backend (Linux Only)
 *
 * This module implements a high-performance network loop using Linux's io_uring
 * interface. It is conditionally compiled and only available on Linux.
 *
 * The loop runs on a single thread. Every connection has exactly one read or
 * write operation queued or in flight at any time, and a connection is only
 * removed (closing its socket) while handling the completion of that
 * operation, so the kernel never touches a buffer that has been freed.
 */

#![cfg(target_os = "linux")]

use crate::net::handle_input;
use crate::protocol::{Request, RequestParser};
use crate::session::Session;
use crate::shard::Shard;
use anyhow::Result;
use bytes::{Buf, BytesMut};
use io_uring::{opcode, squeue, types, IoUring};
use slab::Slab;
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::io::{AsRawFd, FromRawFd};

/// Size of the kernel read buffer of each connection
const READ_BUF: usize = 4096;
/// Submission queue size
const RING_ENTRIES: u32 = 4096;

// user_data = (connection key << 8) | operation tag
const TAG_ACCEPT: u64 = 1;
const TAG_ACCEPT_RETRY: u64 = 2;
const TAG_READ: u64 = 3;
const TAG_WRITE: u64 = 4;

fn user_data(key: usize, tag: u64) -> u64 {
    ((key as u64) << 8) | tag
}

struct Connection {
    /// Owns the socket: dropping the connection closes it.
    stream: TcpStream,
    /// Target of the kernel reads. Boxed so its address stays fixed while a
    /// read is in flight, even when the slab reallocates.
    read_buffer: Box<[u8; READ_BUF]>,
    /// Received bytes not parsed yet
    read_buf: BytesMut,
    /// How far the incomplete request in `read_buf` has been parsed
    parser: RequestParser,
    /// Replies not written yet. Never modified while a write is in flight.
    write_buf: BytesMut,
    reqs: Vec<Request>,
    /// The client's connection state
    session: Session,
    /// Set after a protocol error or QUIT: flush `write_buf`, then close.
    closing: bool,
}

impl Connection {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            read_buffer: Box::new([0u8; READ_BUF]),
            read_buf: BytesMut::with_capacity(READ_BUF),
            parser: RequestParser::new(),
            write_buf: BytesMut::new(),
            reqs: Vec::with_capacity(32),
            session: Session::new(),
            closing: false,
        }
    }

    fn read_entry(&mut self, key: usize) -> squeue::Entry {
        opcode::Read::new(
            types::Fd(self.stream.as_raw_fd()),
            self.read_buffer.as_mut_ptr(),
            READ_BUF as u32,
        )
        .build()
        .user_data(user_data(key, TAG_READ))
    }

    fn write_entry(&self, key: usize) -> squeue::Entry {
        let len = self.write_buf.len().min(u32::MAX as usize) as u32;
        opcode::Write::new(
            types::Fd(self.stream.as_raw_fd()),
            self.write_buf.as_ptr(),
            len,
        )
        .build()
        .user_data(user_data(key, TAG_WRITE))
    }

    /// Queue the next operation after a completion; returns `false` when the
    /// connection is finished and must be removed.
    fn queue_next(&mut self, key: usize, pending: &mut VecDeque<squeue::Entry>) -> bool {
        if !self.write_buf.is_empty() {
            pending.push_back(self.write_entry(key));
        } else if self.closing {
            return false;
        } else {
            pending.push_back(self.read_entry(key));
        }
        true
    }
}

fn accept_entry(listener: types::Fd) -> squeue::Entry {
    // The peer address is not used, so no address buffer is passed.
    opcode::Accept::new(listener, std::ptr::null_mut(), std::ptr::null_mut())
        .flags(libc::SOCK_CLOEXEC)
        .build()
        .user_data(user_data(0, TAG_ACCEPT))
}

fn is_retryable(res: i32) -> bool {
    res == -libc::EINTR || res == -libc::EAGAIN
}

pub fn run_shard(shard_id: usize, addr: SocketAddr, shard: Shard) -> Result<()> {
    println!(
        "🚀 Starting Ignix with io_uring backend (Shard {})",
        shard_id
    );

    let listener = TcpListener::bind(addr)?;
    let listener_fd = types::Fd(listener.as_raw_fd());
    // Pause before accepting again when the process runs out of resources.
    // Must outlive the timeout operations that point at it.
    let backoff = types::Timespec::new().nsec(10_000_000);
    let mut accept_failing = false;
    let mut connections: Slab<Connection> = Slab::with_capacity(1024);
    let mut pending: VecDeque<squeue::Entry> = VecDeque::new();
    // Declared last so it is dropped first, before the buffers and the
    // timespec its operations point to.
    let mut ring = IoUring::new(RING_ENTRIES)?;

    pending.push_back(accept_entry(listener_fd));

    loop {
        submit_pending(&mut ring, &mut pending)?;
        match ring.submit_and_wait(1) {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {}
            Err(e) => return Err(e.into()),
        }

        for cqe in ring.completion() {
            let data = cqe.user_data();
            let res = cqe.result();
            let key = (data >> 8) as usize;

            match data & 0xff {
                TAG_ACCEPT if res >= 0 => {
                    accept_failing = false;
                    // SAFETY: `res` is the descriptor of a socket accept just
                    // created; nothing else owns or closes it.
                    let stream = unsafe { TcpStream::from_raw_fd(res) };
                    stream.set_nodelay(true).ok();
                    let entry = connections.vacant_entry();
                    let key = entry.key();
                    let conn = entry.insert(Connection::new(stream));
                    pending.push_back(conn.read_entry(key));
                    pending.push_back(accept_entry(listener_fd));
                }
                TAG_ACCEPT => {
                    if !accept_failing {
                        log::warn!("accept failed: {}", std::io::Error::from_raw_os_error(-res));
                    }
                    accept_failing = true;
                    if matches!(
                        -res,
                        libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM
                    ) {
                        pending.push_back(
                            opcode::Timeout::new(&backoff)
                                .build()
                                .user_data(user_data(0, TAG_ACCEPT_RETRY)),
                        );
                    } else {
                        pending.push_back(accept_entry(listener_fd));
                    }
                }
                TAG_ACCEPT_RETRY => pending.push_back(accept_entry(listener_fd)),
                TAG_READ => {
                    let keep = match connections.get_mut(key) {
                        Some(conn) if is_retryable(res) => {
                            pending.push_back(conn.read_entry(key));
                            true
                        }
                        // EOF or error. The write buffer is always empty while
                        // a read is in flight, so nothing is left to flush.
                        Some(_) if res <= 0 => false,
                        Some(conn) => {
                            let n = (res as usize).min(READ_BUF);
                            conn.read_buf.extend_from_slice(&conn.read_buffer[..n]);
                            if !handle_input(
                                &shard,
                                &mut conn.session,
                                &mut conn.read_buf,
                                &mut conn.parser,
                                &mut conn.reqs,
                                &mut conn.write_buf,
                            ) {
                                conn.closing = true;
                                conn.read_buf.clear();
                            }
                            conn.queue_next(key, &mut pending)
                        }
                        None => true,
                    };
                    if !keep {
                        connections.remove(key);
                    }
                }
                TAG_WRITE => {
                    let keep = match connections.get_mut(key) {
                        Some(conn) if is_retryable(res) => {
                            pending.push_back(conn.write_entry(key));
                            true
                        }
                        Some(_) if res <= 0 => false,
                        Some(conn) => {
                            let n = (res as usize).min(conn.write_buf.len());
                            conn.write_buf.advance(n);
                            conn.queue_next(key, &mut pending)
                        }
                        None => true,
                    };
                    if !keep {
                        connections.remove(key);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Move queued operations into the submission queue, submitting to the kernel
/// whenever the queue is full.
fn submit_pending(ring: &mut IoUring, pending: &mut VecDeque<squeue::Entry>) -> Result<()> {
    while !pending.is_empty() {
        {
            let mut sq = ring.submission();
            while let Some(entry) = pending.front() {
                // SAFETY: every entry points at memory that outlives the
                // operation: the boxed read buffer or the write buffer of a
                // live connection (removed only when its single in-flight
                // operation completes, and its write buffer is not modified
                // while a write is in flight), or `backoff`, which lives as
                // long as the ring.
                if unsafe { sq.push(entry) }.is_err() {
                    break;
                }
                pending.pop_front();
            }
        }
        if pending.is_empty() {
            break;
        }
        match ring.submit() {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            // The kernel cannot take more work until completions are reaped;
            // the remaining entries are pushed on the next turn of the loop.
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
