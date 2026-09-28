/*!
 * Network Layer and Event Loop
 *
 * This module implements the core networking functionality for Ignix,
 * including the TCP server, connection handling, and the main event loop
 * using mio for async I/O operations.
 */

use crate::protocol::{parse_requests, write_error, Request};
use crate::shard::Shard;
use anyhow::*;
use bytes::{Buf, BytesMut};
use hashbrown::HashMap;
use mio::event::Event;
use mio::net::{TcpListener, TcpStream};
use mio::{Events, Interest, Poll, Registry, Token};
use std::io::{ErrorKind, Read, Write};
use std::net::SocketAddr;
use std::result::Result::{Err, Ok};
use std::sync::Arc;

/// Size of read buffer for incoming data
const READ_BUF: usize = 4096;

/// Whether a short read may end the read loop before `WouldBlock`.
///
/// Linux epoll (edge-triggered, as mio registers sockets) raises a new event
/// for every later arrival of data or FIN, and a read that returns less than
/// the buffer means the receive queue was empty, so the extra read that would
/// only return `EAGAIN` can be skipped. Other platforms re-arm only after
/// `WouldBlock`, so they keep reading until then.
const STOP_AFTER_SHORT_READ: bool = cfg!(target_os = "linux");

use socket2::{Domain, Protocol, Socket, Type};

/// Bind a TCP listener with SO_REUSEPORT support
///
/// Uses socket2 to set SO_REUSEPORT, allowing multiple threads to bind
/// to the same port and share the incoming connection load (kernel load balancing).
pub fn bind_reuseport(addr: SocketAddr) -> Result<TcpListener> {
    let domain = match addr {
        SocketAddr::V4(_) => Domain::IPV4,
        SocketAddr::V6(_) => Domain::IPV6,
    };

    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;

    #[cfg(unix)]
    {
        socket.set_reuse_address(true)?;
        socket.set_reuse_port(true)?;
    }

    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;

    Ok(TcpListener::from_std(socket.into()))
}

/// Run the main server with Multi-Reactor architecture
///
/// Spawns one thread per CPU core. Each thread runs its own event loop
/// and accepts connections on the shared port (via SO_REUSEPORT).
pub fn run_shard(_shard_id: usize, addr: SocketAddr, shard: Shard) -> Result<()> {
    let shard = Arc::new(shard);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    println!(
        "🚀 Starting Ignix with {} worker threads (Multi-Reactor)",
        threads
    );

    let mut handles = Vec::new();

    for id in 0..threads {
        let shard = shard.clone();
        handles.push(std::thread::spawn(move || {
            if let Err(e) = run_worker_loop(id, addr, shard) {
                log::error!("worker {id} stopped: {e:#}");
            }
        }));
    }

    // Wait for all threads (they should run forever)
    for h in handles {
        if h.join().is_err() {
            log::error!("a worker thread panicked");
        }
    }

    Ok(())
}

/// Per-connection state of the mio backend
struct Conn {
    sock: TcpStream,
    /// Received bytes not parsed yet (at most one incomplete request)
    rbuf: BytesMut,
    /// Replies not written to the socket yet
    wbuf: BytesMut,
    /// Parsed requests, reused between reads
    reqs: Vec<Request>,
    /// Set after EOF or a protocol error: stop reading, flush `wbuf`, close
    closing: bool,
    /// Interest currently registered with the poller
    interest: Interest,
}

impl Conn {
    fn new(sock: TcpStream) -> Self {
        Self {
            sock,
            rbuf: BytesMut::with_capacity(READ_BUF),
            wbuf: BytesMut::new(),
            reqs: Vec::with_capacity(32),
            closing: false,
            interest: Interest::READABLE,
        }
    }
}

/// Execute every complete request in `rbuf` and queue the replies in order.
///
/// Invalid commands get an error reply and the connection stays usable.
/// Returns `false` after a protocol error: the requests before it have been
/// executed, its error reply is queued after theirs, and the connection must
/// be closed once `wbuf` is flushed (Redis behaves the same way).
pub(crate) fn handle_input(
    shard: &Shard,
    rbuf: &mut BytesMut,
    reqs: &mut Vec<Request>,
    wbuf: &mut BytesMut,
) -> bool {
    let parsed = parse_requests(rbuf, reqs);
    for req in reqs.drain(..) {
        match req {
            Request::Cmd(cmd) => shard.exec(cmd, wbuf),
            Request::Invalid(message) => write_error(&message, wbuf),
        }
    }
    match parsed {
        Ok(()) => true,
        Err(e) => {
            write_error(&e.to_string(), wbuf);
            false
        }
    }
}

/// Main event loop for a single worker thread
fn run_worker_loop(id: usize, addr: SocketAddr, shard: Arc<Shard>) -> Result<()> {
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(1024);

    // Each worker binds its own listener to the same port (SO_REUSEPORT)
    let mut listener = bind_reuseport(addr)?;

    const LISTENER: Token = Token(0);
    poll.registry()
        .register(&mut listener, LISTENER, Interest::READABLE)?;

    let mut clients: HashMap<usize, Conn> = HashMap::new();
    let mut next_tok: usize = 1;

    // Buffer for reading from socket
    let mut tmp_buf = [0u8; READ_BUF];

    loop {
        if let Err(e) = poll.poll(&mut events, None) {
            if e.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(e.into());
        }

        for ev in events.iter() {
            match ev.token() {
                LISTENER => accept_all(id, poll.registry(), &listener, &mut clients, &mut next_tok),
                Token(t) => {
                    let keep = match clients.get_mut(&t) {
                        Some(conn) => {
                            drive(conn, ev, &shard, &mut tmp_buf, poll.registry(), Token(t))
                        }
                        None => true,
                    };
                    if !keep {
                        // Dropping the stream closes it and removes it from the poller.
                        clients.remove(&t);
                    }
                }
            }
        }
    }
}

/// Accept every pending connection on this worker's listener.
///
/// A connection that cannot be registered is dropped; it never stops the
/// worker.
fn accept_all(
    id: usize,
    registry: &Registry,
    listener: &TcpListener,
    clients: &mut HashMap<usize, Conn>,
    next_tok: &mut usize,
) {
    loop {
        match listener.accept() {
            Ok((mut sock, _)) => {
                sock.set_nodelay(true).ok();
                let tok = *next_tok;
                // Token 0 belongs to the listener.
                *next_tok = next_tok.wrapping_add(1).max(1);
                if let Err(e) = registry.register(&mut sock, Token(tok), Interest::READABLE) {
                    log::warn!("worker {id}: cannot register a new connection: {e}");
                    continue;
                }
                clients.insert(tok, Conn::new(sock));
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::Interrupted | ErrorKind::ConnectionAborted
                ) =>
            {
                continue
            }
            Err(e) => {
                log::warn!("worker {id}: accept failed: {e}");
                break;
            }
        }
    }
}

/// Handle one readiness event of a connection.
///
/// Returns `false` when the connection is finished and must be dropped.
fn drive(
    conn: &mut Conn,
    ev: &Event,
    shard: &Shard,
    tmp_buf: &mut [u8],
    registry: &Registry,
    token: Token,
) -> bool {
    if ev.is_error() {
        return false;
    }

    if (ev.is_readable() || ev.is_read_closed()) && !conn.closing {
        // A FIN that arrived together with data is reported in this event
        // only, so after the peer closed keep reading until EOF.
        let peer_closed = ev.is_read_closed();
        // Edge-triggered: read until the socket is drained.
        loop {
            match conn.sock.read(tmp_buf) {
                Ok(0) => {
                    conn.closing = true;
                    break;
                }
                Ok(n) => {
                    conn.rbuf.extend_from_slice(&tmp_buf[..n]);
                    // A short read drained the socket (TCP urgent data, which
                    // RESP clients never send, is the only exception).
                    if STOP_AFTER_SHORT_READ && n < tmp_buf.len() && !peer_closed {
                        break;
                    }
                }
                Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(ref e) if would_block(e) => break,
                Err(_) => return false,
            }
        }
        // Run what arrived even after EOF: a client may send its requests and
        // half-close right away, and still read the replies.
        if !handle_input(shard, &mut conn.rbuf, &mut conn.reqs, &mut conn.wbuf) {
            conn.closing = true;
            conn.rbuf.clear();
        }
    }

    // Write until everything is sent or the socket is full.
    while !conn.wbuf.is_empty() {
        match conn.sock.write(&conn.wbuf) {
            Ok(0) => return false,
            Ok(n) => conn.wbuf.advance(n),
            Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(ref e) if would_block(e) => break,
            Err(_) => return false,
        }
    }

    if conn.closing && conn.wbuf.is_empty() {
        return false;
    }

    let wanted = match (conn.closing, conn.wbuf.is_empty()) {
        (false, true) => Interest::READABLE,
        (false, false) => Interest::READABLE | Interest::WRITABLE,
        // Closing: stop reading, only flush the remaining replies.
        (true, _) => Interest::WRITABLE,
    };
    if wanted != conn.interest {
        if registry.reregister(&mut conn.sock, token, wanted).is_err() {
            return false;
        }
        conn.interest = wanted;
    }
    true
}

/// Check if an I/O error indicates the operation would block
#[inline]
fn would_block(e: &std::io::Error) -> bool {
    e.kind() == ErrorKind::WouldBlock
}
