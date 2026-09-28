/*!
 * Network Layer and Event Loop
 *
 * This module implements the core networking functionality for Ignix,
 * including the TCP server, connection handling, and the main event loop
 * using mio for async I/O operations.
 */

use crate::protocol::{write_error, Request, RequestParser};
use crate::session::Session;
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
use std::time::{Duration, Instant};

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

/// Default busy-poll window of [`ServerOptions`]
pub const DEFAULT_BUSY_POLL: Duration = Duration::from_micros(50);

/// Tuning options for [`run_server`]
///
/// Start from [`ServerOptions::default`] and change the fields you need.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ServerOptions {
    /// How long a worker keeps polling for new events without sleeping after
    /// its last event (default [`DEFAULT_BUSY_POLL`]); zero disables it.
    ///
    /// Waking a sleeping worker is expensive: the client that sends the next
    /// request pays for the wake-up, and on virtual machines an idle vCPU has
    /// to be woken as well. Polling briefly keeps latency low while traffic
    /// flows, at the cost of CPU time under load. An idle server sleeps.
    pub busy_poll: Duration,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            busy_poll: DEFAULT_BUSY_POLL,
        }
    }
}

/// Run the main server with Multi-Reactor architecture and default options
///
/// See [`run_server`].
pub fn run_shard(_shard_id: usize, addr: SocketAddr, shard: Shard) -> Result<()> {
    run_server(addr, shard, ServerOptions::default())
}

/// Run the main server with Multi-Reactor architecture
///
/// Spawns one thread per CPU core. Each thread runs its own event loop
/// and accepts connections on the shared port (via SO_REUSEPORT).
pub fn run_server(addr: SocketAddr, shard: Shard, options: ServerOptions) -> Result<()> {
    let shard = Arc::new(shard);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    // Bind every worker's listener before starting any worker, so that an
    // address that cannot be used is an error instead of a running server
    // that accepts nothing.
    let listeners = (0..threads)
        .map(|_| bind_reuseport(addr))
        .collect::<Result<Vec<_>>>()
        .with_context(|| format!("cannot listen on {addr}"))?;

    println!(
        "🚀 Starting Ignix with {} worker threads (Multi-Reactor)",
        threads
    );

    let mut handles = Vec::new();

    for (id, listener) in listeners.into_iter().enumerate() {
        let shard = shard.clone();
        let busy_poll = options.busy_poll;
        handles.push(std::thread::spawn(move || {
            if let Err(e) = run_worker_loop(id, listener, shard, busy_poll) {
                log::error!("worker {id} stopped: {e:#}");
            }
        }));
    }

    // Workers run until they fail
    for h in handles {
        if h.join().is_err() {
            log::error!("a worker thread panicked");
        }
    }

    bail!("every worker thread has stopped")
}

/// Per-connection state of the mio backend
struct Conn {
    sock: TcpStream,
    /// Received bytes not parsed yet (at most one incomplete request)
    rbuf: BytesMut,
    /// How far the incomplete request in `rbuf` has been parsed
    parser: RequestParser,
    /// Replies not written to the socket yet
    wbuf: BytesMut,
    /// Parsed requests, reused between reads
    reqs: Vec<Request>,
    /// The client's connection state
    session: Session,
    /// Set after EOF, a protocol error or QUIT: stop reading, flush `wbuf`,
    /// close
    closing: bool,
    /// Interest currently registered with the poller
    interest: Interest,
}

impl Conn {
    fn new(sock: TcpStream) -> Self {
        Self {
            sock,
            rbuf: BytesMut::with_capacity(READ_BUF),
            parser: RequestParser::new(),
            wbuf: BytesMut::new(),
            reqs: Vec::with_capacity(32),
            session: Session::new(),
            closing: false,
            interest: Interest::READABLE,
        }
    }
}

/// Execute every complete request in `rbuf` and queue the replies in order.
///
/// Invalid commands get an error reply and the connection stays usable.
/// Returns `false` when the connection must be closed once `wbuf` is
/// flushed, as Redis does: after a protocol error, whose error reply is
/// queued after the replies of the requests before it, and after `QUIT`,
/// whose reply is the last one (later requests are dropped). `parser` keeps
/// the progress through a request that has not fully arrived; the caller
/// only appends to `rbuf`.
pub(crate) fn handle_input(
    shard: &Shard,
    session: &mut Session,
    rbuf: &mut BytesMut,
    parser: &mut RequestParser,
    reqs: &mut Vec<Request>,
    wbuf: &mut BytesMut,
) -> bool {
    let parsed = parser.parse(rbuf, reqs);
    for req in reqs.drain(..) {
        match req {
            Request::Cmd(cmd) => shard.exec_session(cmd, session, wbuf),
            Request::Invalid(message) => write_error(&message, wbuf),
        }
        if session.is_closing() {
            // Dropping the iterator drops the requests after QUIT
            break;
        }
    }
    if session.is_closing() {
        parser.reset();
        return false;
    }
    match parsed {
        Ok(()) => true,
        Err(e) => {
            write_error(&e.to_string(), wbuf);
            parser.reset();
            false
        }
    }
}

/// Timeout for the next poll: zero while the busy-poll window after the last
/// event is open, otherwise block until the next event.
fn poll_timeout(busy_poll: Duration, idle_for: Option<Duration>) -> Option<Duration> {
    match idle_for {
        _ if busy_poll.is_zero() => None,
        Some(idle) if idle >= busy_poll => None,
        _ => Some(Duration::ZERO),
    }
}

/// Main event loop for a single worker thread
///
/// Each worker has its own listener bound to the same port (SO_REUSEPORT).
fn run_worker_loop(
    id: usize,
    mut listener: TcpListener,
    shard: Arc<Shard>,
    busy_poll: Duration,
) -> Result<()> {
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(1024);

    const LISTENER: Token = Token(0);
    poll.registry()
        .register(&mut listener, LISTENER, Interest::READABLE)?;

    let mut clients: HashMap<usize, Conn> = HashMap::new();
    let mut next_tok: usize = 1;

    // Buffer for reading from socket
    let mut tmp_buf = [0u8; READ_BUF];

    // When the last event was handled, while no new one has arrived since
    let mut idle_since: Option<Instant> = None;

    loop {
        let timeout = poll_timeout(busy_poll, idle_since.map(|since| since.elapsed()));
        if let Err(e) = poll.poll(&mut events, timeout) {
            if e.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(e.into());
        }
        if events.is_empty() {
            if timeout.is_some() {
                idle_since.get_or_insert_with(Instant::now);
            }
            continue;
        }
        idle_since = None;

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
        if !handle_input(
            shard,
            &mut conn.session,
            &mut conn.rbuf,
            &mut conn.parser,
            &mut conn.reqs,
            &mut conn.wbuf,
        ) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_server_fails_when_it_cannot_listen() {
        // A listener without SO_REUSEPORT keeps the port to itself
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = taken.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = run_server(addr, Shard::new(0, None), ServerOptions::default());
            let _ = tx.send(result.is_err());
        });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)),
            Ok(true),
            "run_server must return an error when the address is in use"
        );
        drop(taken);
    }

    fn request(args: &[&[u8]]) -> Vec<u8> {
        let mut out = format!("*{}\r\n", args.len()).into_bytes();
        for arg in args {
            out.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
            out.extend_from_slice(arg);
            out.extend_from_slice(b"\r\n");
        }
        out
    }

    /// Run `input` through `handle_input` on a new connection; returns
    /// whether the connection stays open and the replies.
    fn run_input(shard: &Shard, input: &[u8]) -> (bool, Vec<u8>) {
        let mut session = Session::new();
        let mut rbuf = BytesMut::from(input);
        let mut wbuf = BytesMut::new();
        let open = handle_input(
            shard,
            &mut session,
            &mut rbuf,
            &mut RequestParser::new(),
            &mut Vec::new(),
            &mut wbuf,
        );
        (open, wbuf.to_vec())
    }

    #[test]
    fn requests_after_quit_are_dropped() {
        let shard = Shard::new(0, None);
        let mut input = request(&[b"PING"]);
        input.extend(request(&[b"QUIT"]));
        input.extend(request(&[b"SET", b"k", b"v"]));
        input.extend(request(&[b"PING"]));
        assert_eq!(
            run_input(&shard, &input),
            (false, b"+PONG\r\n+OK\r\n".to_vec())
        );
        assert_eq!(shard.dict.get(b"k"), None);
    }

    #[test]
    fn malformed_input_after_quit_gets_no_error_reply() {
        let shard = Shard::new(0, None);
        let mut input = request(&[b"QUIT"]);
        input.extend_from_slice(b"*1\r\nX");
        assert_eq!(run_input(&shard, &input), (false, b"+OK\r\n".to_vec()));
    }

    #[test]
    fn workers_poll_without_blocking_only_inside_the_busy_poll_window() {
        let window = Duration::from_micros(50);
        // Right after an event, and while the window is open: do not block
        assert_eq!(poll_timeout(window, None), Some(Duration::ZERO));
        assert_eq!(
            poll_timeout(window, Some(Duration::from_micros(10))),
            Some(Duration::ZERO)
        );
        // The window has passed without events: sleep until the next one
        assert_eq!(poll_timeout(window, Some(window)), None);
        // Busy-polling disabled: always sleep
        assert_eq!(poll_timeout(Duration::ZERO, None), None);
    }
}
