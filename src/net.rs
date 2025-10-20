/*!
 * Network Layer and Event Loop
 * 
 * This module implements the core networking functionality for Ignix,
 * including the TCP server, connection handling, and the main event loop
 * using mio for async I/O operations.
 */

use crate::protocol::{parse_many, resp_simple};
use crate::shard::Shard;
use anyhow::*;
use bytes::BytesMut;
use hashbrown::HashMap;
use mio::net::{TcpListener, TcpStream};
use mio::{Events, Interest, Poll, Token};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::result::Result::{Ok, Err};
use std::time::Duration;

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
pub fn run_shard(_shard_id: usize, addr: SocketAddr, mut shard: Shard) -> Result<()> {
    // Create the main event loop components
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(1024);
    let mut listener = bind_reuseport(addr)?;
    
    // Register the listener socket for accepting new connections
    poll.registry()
        .register(&mut listener, Token(0), Interest::READABLE)?;

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
                // Token(0) = listening socket, accept new connections
                Token(0) => loop {
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
                                    // Execute each parsed command
                                    for c in cmds {
                                        let resp = shard.exec(c);
                                        wbuf.extend_from_slice(&resp);
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