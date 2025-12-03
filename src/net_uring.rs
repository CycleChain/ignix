/*!
 * io_uring Network Backend (Linux Only)
 * 
 * This module implements a high-performance network loop using Linux's io_uring
 * interface. It is conditionally compiled and only available on Linux.
 */

#![cfg(target_os = "linux")]

use crate::shard::Shard;
use crate::protocol::{parse_many, write_simple, Cmd};
use anyhow::*;
use bytes::BytesMut;
use io_uring::{opcode, types, IoUring};
use slab::Slab;
use std::net::SocketAddr;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use std::net::TcpListener;

// Operation types for user_data
const OP_ACCEPT: u64 = 0;
// User data structure: (token << 32) | op_type
// where op_type: 1 = READ, 2 = WRITE

#[derive(Debug)]
struct Connection {
    fd: i32,
    // Box provides stable address for io_uring even if Slab reallocates
    read_buffer: Box<[u8; 4096]>, 
    read_buf: BytesMut,
    write_buf: BytesMut,
    cmds: Vec<Cmd>,
}

pub fn run_shard(shard_id: usize, addr: SocketAddr, shard: Shard) -> Result<()> {
    println!("🚀 Starting Ignix with io_uring backend (Shard {})", shard_id);
    
    // Setup listener
    let listener = TcpListener::bind(addr)?;
    let listener_fd = listener.as_raw_fd();

    // Setup io_uring
    let mut ring = IoUring::new(4096)?;
    let mut connections = Slab::with_capacity(1024);

    // Initial Accept
    let mut accept_addr = libc::sockaddr { sa_family: 0, sa_data: [0; 14] };
    let mut accept_addr_len: libc::socklen_t = std::mem::size_of::<libc::sockaddr>() as _;

    {
        let mut sq = ring.submission();
        let accept_op = opcode::Accept::new(
            types::Fd(listener_fd),
            &mut accept_addr,
            &mut accept_addr_len
        )
        .build()
        .user_data(OP_ACCEPT);
        
        unsafe {
            sq.push(&accept_op).expect("submission queue full");
        }
        sq.sync();
    }

    loop {
        ring.submit_and_wait(1)?;

        let mut cq = ring.completion();
        let mut sq = ring.submission();

        for cqe in cq {
            let user_data = cqe.user_data();
            let res = cqe.result();

            if user_data == OP_ACCEPT {
                if res < 0 {
                    eprintln!("Accept error: {}", res);
                } else {
                    let fd = res;
                    let entry = connections.vacant_entry();
                    let key = entry.key();
                    
                    let mut conn = Connection {
                        fd,
                        read_buffer: Box::new([0u8; 4096]),
                        read_buf: BytesMut::with_capacity(4096),
                        write_buf: BytesMut::new(),
                        cmds: Vec::new(),
                    };
                    
                    // Get stable pointer before moving conn into Slab
                    // Actually, Box pointer is stable even after move.
                    let buf_ptr = conn.read_buffer.as_mut_ptr();
                    let buf_len = conn.read_buffer.len();

                    entry.insert(conn);

                    // Re-submit Accept
                    let accept_op = opcode::Accept::new(
                        types::Fd(listener_fd),
                        &mut accept_addr,
                        &mut accept_addr_len
                    )
                    .build()
                    .user_data(OP_ACCEPT);
                    
                    unsafe {
                        sq.push(&accept_op).expect("sq full");
                    }
                    
                    // Submit Read
                    let read_op = opcode::Read::new(
                        types::Fd(fd),
                        buf_ptr,
                        buf_len as _
                    )
                    .build()
                    .user_data(((key as u64) << 32) | 1); // 1 = READ

                    unsafe {
                        sq.push(&read_op).expect("sq full");
                    }
                }
            } else {
                let key = (user_data >> 32) as usize;
                let op = user_data & 0xFFFFFFFF;

                if connections.contains(key) {
                    if op == 1 { // READ completion
                        if res <= 0 {
                            // EOF or Error
                            connections.remove(key);
                            // Close FD - handled by Drop? No, need manual close or impl Drop
                            // unsafe { libc::close(conn.fd); }
                        } else {
                            let conn = connections.get_mut(key).unwrap();
                            conn.read_buf.extend_from_slice(&conn.read_buffer[..res as usize]);
                            
                            // Parse and Execute
                            if let Ok(_) = parse_many(&mut conn.read_buf, &mut conn.cmds) {
                                for cmd in conn.cmds.drain(..) {
                                    shard.exec(cmd, &mut conn.write_buf);
                                }
                            }

                            // Submit Write if needed
                            if !conn.write_buf.is_empty() {
                                let write_op = opcode::Write::new(
                                    types::Fd(conn.fd),
                                    conn.write_buf.as_ptr(),
                                    conn.write_buf.len() as _
                                )
                                .build()
                                .user_data(((key as u64) << 32) | 2); // 2 = WRITE
                                
                                unsafe {
                                    sq.push(&write_op).expect("sq full");
                                }
                            } else {
                                // Continue Reading
                                let read_op = opcode::Read::new(
                                    types::Fd(conn.fd),
                                    conn.read_buffer.as_mut_ptr(),
                                    conn.read_buffer.len() as _
                                )
                                .build()
                                .user_data(((key as u64) << 32) | 1);

                                unsafe {
                                    sq.push(&read_op).expect("sq full");
                                }
                            }
                        }
                    } else if op == 2 { // WRITE completion
                         if res < 0 {
                            connections.remove(key);
                        } else {
                            let conn = connections.get_mut(key).unwrap();
                            let _ = conn.write_buf.split_to(res as usize);

                            if !conn.write_buf.is_empty() {
                                // Continue Writing
                                let write_op = opcode::Write::new(
                                    types::Fd(conn.fd),
                                    conn.write_buf.as_ptr(),
                                    conn.write_buf.len() as _
                                )
                                .build()
                                .user_data(((key as u64) << 32) | 2);
                                
                                unsafe {
                                    sq.push(&write_op).expect("sq full");
                                }
                            } else {
                                // Back to Reading
                                let read_op = opcode::Read::new(
                                    types::Fd(conn.fd),
                                    conn.read_buffer.as_mut_ptr(),
                                    conn.read_buffer.len() as _
                                )
                                .build()
                                .user_data(((key as u64) << 32) | 1);

                                unsafe {
                                    sq.push(&read_op).expect("sq full");
                                }
                            }
                        }
                    }
                }
            }
        }
        
        sq.sync();
    }
}
