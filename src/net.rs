/*!
 * Network Layer and Event Loop
 * 
 * This module implements the core networking functionality for Ignix,
 * including the TCP server, connection handling, and the main event loop
 * using mio for async I/O operations.
 */

use crate::protocol::{parse_many, write_simple, Cmd};
use crate::shard::Shard;
use anyhow::*;
use bytes::BytesMut;
use hashbrown::HashMap;
use mio::net::{TcpListener, TcpStream};
use mio::{Events, Interest, Poll, Token};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::result::Result::{Ok, Err};
use std::sync::Arc;

/// Size of read buffer for incoming data
const READ_BUF: usize = 4096;

use socket2::{Socket, Domain, Type, Protocol};

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
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    
    println!("🚀 Starting Ignix with {} worker threads (Multi-Reactor)", threads);
    
    let mut handles = Vec::new();
    
    for id in 0..threads {
        let shard = shard.clone();
        let addr = addr;
        handles.push(std::thread::spawn(move || {
            if let Err(e) = run_worker_loop(id, addr, shard) {
                eprintln!("Worker {} failed: {}", id, e);
            }
        }));
    }
    
    // Wait for all threads (they should run forever)
    for h in handles {
        h.join().unwrap();
    }
    
    Ok(())
}

/// Main event loop for a single worker thread
fn run_worker_loop(id: usize, addr: SocketAddr, shard: Arc<Shard>) -> Result<()> {
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(1024);
    
    // Each worker binds its own listener to the same port (SO_REUSEPORT)
    let mut listener = bind_reuseport(addr)?;
    
    const LISTENER: Token = Token(0);
    poll.registry().register(&mut listener, LISTENER, Interest::READABLE)?;
    
    // Client state: (socket, read_buf, write_buf, cmd_buf)
    let mut clients: HashMap<usize, (TcpStream, BytesMut, BytesMut, Vec<Cmd>)> = HashMap::new();
    let mut next_tok: usize = 1;
    
    // Buffer for reading from socket
    let mut tmp_buf = [0u8; READ_BUF];

    loop {
        poll.poll(&mut events, None)?;
        
        for ev in events.iter() {
            match ev.token() {
                LISTENER => loop {
                    match listener.accept() {
                        Ok((mut sock, _)) => {
                            sock.set_nodelay(true).ok();
                            let tok = next_tok;
                            next_tok = next_tok.wrapping_add(1);
                            if next_tok == 0 { next_tok = 1; } // Skip 0 (LISTENER)

                            // Register client socket for READABLE only initially
                            poll.registry().register(
                                &mut sock,
                                Token(tok),
                                Interest::READABLE,
                            )?;
                            
                            // println!("Worker {} accepted connection {}", id, tok);
                            clients.insert(tok, (sock, BytesMut::with_capacity(READ_BUF), BytesMut::new(), Vec::with_capacity(32)));
                        }
                        Err(ref e) if would_block(e) => break,
                        Err(e) => {
                            eprintln!("Worker {} accept err: {}", id, e);
                            break;
                        }
                    }
                },
                Token(t) => {
                    let mut should_remove = false;
                    if let Some((sock, rbuf, wbuf, cmds)) = clients.get_mut(&t) {
                        // READ
                        if ev.is_readable() {
                            loop {
                                match sock.read(&mut tmp_buf) {
                                    Ok(0) => { should_remove = true; break; }
                                    Ok(n) => {
                                        rbuf.extend_from_slice(&tmp_buf[..n]);
                                    }
                                    Err(ref e) if would_block(e) => break,
                                    Err(_) => { should_remove = true; break; }
                                }
                            }
                            
                            // PARSE & EXECUTE (Inline)
                            if !should_remove {
                                cmds.clear();
                                if let Err(e) = parse_many(rbuf, cmds) {
                                    write_simple(&format!("ERR {}", e), wbuf);
                                } else {
                                    for cmd in cmds.drain(..) {
                                        shard.exec(cmd, wbuf);
                                    }
                                }
                                
                                // Try to write immediately
                                if !wbuf.is_empty() {
                                    match sock.write(wbuf) {
                                        Ok(n) => { let _ = wbuf.split_to(n); }
                                        Err(ref e) if would_block(e) => {}
                                        Err(_) => { should_remove = true; }
                                    }
                                }
                            }
                        }
                        
                        // WRITE
                        if !should_remove && ev.is_writable() && !wbuf.is_empty() {
                            match sock.write(wbuf) {
                                Ok(n) => { let _ = wbuf.split_to(n); }
                                Err(ref e) if would_block(e) => {}
                                Err(_) => { should_remove = true; }
                            }
                        }
                        
                        // Update Interest based on wbuf state
                        if !should_remove {
                            let interest = if wbuf.is_empty() {
                                Interest::READABLE
                            } else {
                                Interest::READABLE | Interest::WRITABLE
                            };
                            
                            if let Err(_) = poll.registry().reregister(sock, Token(t), interest) {
                                should_remove = true;
                            }
                        }
                    }
                    
                    if should_remove {
                        clients.remove(&t);
                    }
                }
            }
        }
    }
}

/// Check if an I/O error indicates the operation would block
#[inline]
fn would_block(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    )
}