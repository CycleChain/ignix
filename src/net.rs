/*!
 * Network Layer and Event Loop
 * 
 * This module implements the core networking functionality for Ignix,
 * including the TCP server, connection handling, and the main event loop
 * using mio for async I/O operations.
 */

use crate::protocol::{parse_many, resp_simple, Cmd};
use crate::shard::Shard;
use anyhow::*;
use bytes::BytesMut;
use crossbeam::channel::{bounded, Receiver, Sender, TryRecvError};
use hashbrown::HashMap;
use mio::net::{TcpListener, TcpStream};
use mio::{Events, Interest, Poll, Token, Waker};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::result::Result::{Ok, Err};
use std::time::Duration;
use std::sync::Arc;

/// Size of read buffer for incoming data
const READ_BUF: usize = 4096;

/// Bind a TCP listener with potential SO_REUSEPORT support
/// 
/// Currently uses standard mio TcpListener binding. SO_REUSEPORT
/// support can be added later for better multi-process scaling.
/// 
/// # Arguments
/// * `addr` - Socket address to bind to
/// 
/// # Returns
/// * Bound TcpListener ready for accepting connections
pub fn bind_reuseport(addr: SocketAddr) -> Result<TcpListener> {
    // For now, let's use the simpler mio TcpListener::bind
    // TODO: Add SO_REUSEPORT support for better scaling
    Ok(TcpListener::bind(addr)?)
}

/// Run the main server event loop for a shard
/// 
/// This is the core of the Ignix server - an async event loop that handles
/// all client connections, command parsing, and response writing using mio
/// for high-performance non-blocking I/O.
/// 
/// # Arguments
/// * `shard_id` - Identifier for this shard (currently unused)
/// * `addr` - Address to bind the server to
/// * `shard` - Shard instance to execute commands on
/// 
/// # Architecture
/// * Uses Token(0) for the listening socket
/// * Each client gets a unique token starting from 1
/// * Maintains read/write buffers for each client
/// * Processes commands immediately when complete
pub fn run_shard(_shard_id: usize, addr: SocketAddr, shard: Shard) -> Result<()> {
    // Create the main event loop components
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(1024);
    let mut listener = bind_reuseport(addr)?;
    const LISTENER: Token = Token(0);
    const WAKER_TOKEN: Token = Token(usize::MAX - 1);
    
    // Register the listener socket for accepting new connections
    poll.registry()
        .register(&mut listener, LISTENER, Interest::READABLE)?;

    // Channels for offloading command execution to worker threads
    let (tx_task, rx_task): (Sender<(usize, Cmd)>, Receiver<(usize, Cmd)>) = bounded(1024);
    let (tx_resp, rx_resp): (Sender<(usize, Vec<u8>)>, Receiver<(usize, Vec<u8>)>) = bounded(1024);

    // Waker to notify reactor when responses are ready
    let waker = Arc::new(Waker::new(poll.registry(), WAKER_TOKEN)?);

    // Shared shard for workers (thread-safe storage inside)
    let shard = Arc::new(shard);

    // Spawn worker threads
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    for _ in 0..workers {
        let rx_task_cl = rx_task.clone();
        let tx_resp_cl = tx_resp.clone();
        let shard_cl = Arc::clone(&shard);
        let waker_cl = Arc::clone(&waker);
        std::thread::spawn(move || {
            while let Ok((tok, cmd)) = rx_task_cl.recv() {
                let resp = shard_cl.exec(cmd);
                // Best-effort send back response
                if tx_resp_cl.send((tok, resp)).is_ok() {
                    let _ = waker_cl.wake();
                }
            }
        });
    }

    // Client connection storage: token -> (socket, read_buffer, write_buffer)
    // Using SwissTable for better performance than std HashMap
    let mut clients: HashMap<usize, (TcpStream, BytesMut, BytesMut)> = HashMap::new();
    let mut next_tok: usize = 1;

    // Main event loop
    loop {
        // Poll for events with 200ms timeout
        poll.poll(&mut events, Some(Duration::from_millis(200)))?;
        
        for ev in events.iter() {
            match ev.token() {
                // Listener socket, accept new connections
                LISTENER => loop {
                    match listener.accept() {
                        Ok((mut sock, _)) => {
                            // Enable TCP_NODELAY for lower latency
                            sock.set_nodelay(true).ok();
                            
                            // Assign unique token to this client
                            let tok = next_tok;
                            next_tok += 1;
                            
                            // Register client socket for read/write events
                            poll.registry().register(
                                &mut sock,
                                Token(tok),
                                Interest::READABLE | Interest::WRITABLE,
                            )?;
                            
                            // Store client with read/write buffers
                            clients.insert(
                                tok,
                                (sock, BytesMut::with_capacity(READ_BUF), BytesMut::new()),
                            );
                        }
                        // No more connections to accept right now
                        Err(ref e) if would_block(e) => break,
                        Err(e) => {
                            eprintln!("accept err: {e}");
                            break;
                        }
                    }
                },
                
                // Waker notifications from workers
                WAKER_TOKEN => {
                    // Drain all available responses and try to flush immediately
                    loop {
                        match rx_resp.try_recv() {
                            Ok((token_usize, out)) => {
                                if let Some((sock, _r, w)) = clients.get_mut(&token_usize) {
                                    w.extend_from_slice(&out);
                                    if !w.is_empty() {
                                        match sock.write(&w) {
                                            Ok(n) => {
                                                let _ = w.split_to(n);
                                            }
                                            Err(ref e) if would_block(e) => {}
                                            Err(_) => {
                                                // ignore here; regular writable path will clean up
                                            }
                                        }
                                    }
                                }
                            }
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => break,
                        }
                    }
                }

                // Client socket events
                Token(t) => {
                    let mut should_remove = false;
                    
                    if let Some((sock, rbuf, wbuf)) = clients.get_mut(&t) {
                        // Handle readable events (incoming data)
                        if ev.is_readable() {
                            let mut tmp = [0u8; READ_BUF];
                            
                            // Read all available data
                            loop {
                                match sock.read(&mut tmp) {
                                    // Connection closed by client
                                    Ok(0) => {
                                        should_remove = true;
                                        break;
                                    }
                                    // Data received, add to read buffer
                                    Ok(n) => {
                                        rbuf.extend_from_slice(&tmp[..n]);
                                    }
                                    // No more data available right now
                                    Err(ref e) if would_block(e) => break,
                                    // Connection error
                                    Err(_) => {
                                        should_remove = true;
                                        break;
                                    }
                                }
                            }
                            
                            // Process any complete commands in the read buffer
                            if !should_remove {
                                let mut cmds = Vec::new();
                                
                                // Try to parse RESP commands from buffer
                                if let Err(e) = parse_many(rbuf, &mut cmds) {
                                    // Protocol error, send error response
                                    wbuf.extend_from_slice(&resp_simple(&format!("ERR {}", e)));
                                } else {
                                    // Offload each parsed command to workers
                                    for c in cmds {
                                        match tx_task.try_send((t, c)) {
                                            Ok(_) => {}
                                            Err(_) => {
                                                // Backpressure: queue a busy error
                                                wbuf.extend_from_slice(b"-ERR server busy\r\n");
                                            }
                                        }
                                    }
                                }
                                
                                // Try to write response immediately if we have data
                                if !wbuf.is_empty() {
                                    match sock.write(&wbuf) {
                                        Ok(n) => {
                                            // Remove written bytes from buffer
                                            let _ = wbuf.split_to(n);
                                        }
                                        // Socket not ready for writing, will retry later
                                        Err(ref e) if would_block(e) => {
                                            // Will retry on writable event
                                        }
                                        // Write error
                                        Err(_) => {
                                            should_remove = true;
                                        }
                                    }
                                }
                            }
                        }
                        
                        // Handle writable events (can send more data)
                        if !should_remove && ev.is_writable() && !wbuf.is_empty() {
                            match sock.write(&wbuf) {
                                Ok(n) => {
                                    // Remove written bytes from buffer
                                    let _ = wbuf.split_to(n);
                                }
                                // Socket not ready, will retry later
                                Err(ref e) if would_block(e) => {}
                                // Write error
                                Err(_) => {
                                    should_remove = true;
                                }
                            }
                        }
                    }
                    
                    // Clean up disconnected clients
                    if should_remove {
                        clients.remove(&t);
                    }
                }
            }
        }
    }
}

/// Check if an I/O error indicates the operation would block
/// 
/// Helper function to identify non-blocking I/O conditions that
/// should be retried later rather than treated as errors.
#[inline]
fn would_block(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    )
}