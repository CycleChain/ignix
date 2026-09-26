/*!
 * Append-Only File (AOF) Persistence
 *
 * This module implements Redis-compatible AOF persistence for durability.
 * Commands are logged in RESP format to a file and periodically flushed
 * to disk for crash recovery.
 */

use crate::protocol::fmt_u64;
use anyhow::*;
use crossbeam::channel::{bounded, Sender};
use std::io::Write;
use std::result::Result::{Err, Ok};
use std::time::{Duration, Instant};

/// Handle for writing to the AOF (Append-Only File)
///
/// This handle allows async writing to the AOF file through a background
/// thread. Commands are sent via a channel and written to disk periodically.
#[derive(Clone)]
pub struct AofHandle {
    /// Channel sender for sending commands to the AOF writer thread
    tx: Sender<Vec<u8>>,
}

/// Spawn a background AOF writer thread
///
/// Creates a dedicated thread that handles all AOF writes asynchronously.
/// This prevents blocking the main execution thread on disk I/O operations.
///
/// # Arguments
/// * `path` - File path for the AOF file
///
/// # Returns
/// * `AofHandle` for sending commands to be logged
///
/// # Behavior
/// * Commands are buffered and written to disk
/// * File is flushed and synced every 1000ms for durability
/// * Thread continues until the handle is dropped
pub fn spawn_aof_writer(path: &str) -> Result<AofHandle> {
    // Bounded channel to provide backpressure under heavy write load
    let (tx, rx) = bounded::<Vec<u8>>(4096);
    let path = path.to_string();

    // Spawn dedicated AOF writer thread
    std::thread::Builder::new()
        .name("aof-writer".into())
        .spawn(move || {
            // Open AOF file in append mode, create if doesn't exist
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .expect("open aof");

            let mut last = Instant::now();

            // Main AOF writer loop
            loop {
                match rx.recv() {
                    Ok(buf) => {
                        let _ = f.write_all(&buf);
                        if last.elapsed() >= Duration::from_millis(1000) {
                            let _ = f.flush();
                            let _ = f.sync_data();
                            last = Instant::now();
                        }
                    }
                    // Channel closed: drain finished; perform final flush and exit
                    Err(_) => {
                        let _ = f.flush();
                        let _ = f.sync_data();
                        break;
                    }
                }
            }
        })?;

    Ok(AofHandle { tx })
}

impl AofHandle {
    /// Write a command to the AOF
    ///
    /// Sends the command bytes to the background writer thread.
    /// This is non-blocking and returns immediately.
    ///
    /// # Arguments
    /// * `bytes` - RESP-formatted command bytes to write
    #[inline]
    pub fn write(&self, bytes: &[u8]) {
        // Send to background thread, ignore errors (channel closed)
        let _ = self.tx.send(bytes.to_vec());
    }
}

//
// AOF Command Emission Functions
//
// These functions encode commands as RESP arrays of bulk strings, the format
// clients send and Redis AOF files use. Keys and values are copied byte for
// byte, so binary data is preserved.
//

/// Number of decimal digits of `n`.
fn decimal_len(n: usize) -> usize {
    n.checked_ilog10().map_or(1, |digits| digits as usize + 1)
}

/// Append a `<prefix><n>\r\n` header line.
fn push_header(out: &mut Vec<u8>, prefix: u8, n: usize) {
    let mut digits = [0u8; 20];
    out.push(prefix);
    out.extend_from_slice(fmt_u64(n as u64, &mut digits));
    out.extend_from_slice(b"\r\n");
}

/// Append a `$<len>\r\n<bytes>\r\n` bulk string.
fn push_bulk(out: &mut Vec<u8>, bytes: &[u8]) {
    push_header(out, b'$', bytes.len());
    out.extend_from_slice(bytes);
    out.extend_from_slice(b"\r\n");
}

/// Encoded size of a bulk string holding `bytes`.
fn bulk_len(bytes: &[u8]) -> usize {
    1 + decimal_len(bytes.len()) + 2 + bytes.len() + 2
}

/// Encode the command `name args...` into an exactly sized buffer.
fn encode<'a>(name: &[u8], args: impl Iterator<Item = &'a [u8]> + Clone) -> Vec<u8> {
    let count = 1 + args.clone().count();
    let size =
        1 + decimal_len(count) + 2 + bulk_len(name) + args.clone().map(bulk_len).sum::<usize>();
    let mut out = Vec::with_capacity(size);
    push_header(&mut out, b'*', count);
    push_bulk(&mut out, name);
    for arg in args {
        push_bulk(&mut out, arg);
    }
    out
}

/// Generate AOF entry for SET command
///
/// Format: *3\r\n$3\r\nSET\r\n$<keylen>\r\n<key>\r\n$<vallen>\r\n<val>\r\n
///
/// # Arguments
/// * `k` - Key bytes
/// * `v` - Value bytes
pub fn emit_aof_set(k: &[u8], v: &[u8]) -> Vec<u8> {
    encode(b"SET", [k, v].into_iter())
}

/// Generate AOF entry for RENAME command
///
/// # Arguments
/// * `a` - Old key bytes
/// * `b` - New key bytes
pub fn emit_aof_rename(a: &[u8], b: &[u8]) -> Vec<u8> {
    encode(b"RENAME", [a, b].into_iter())
}

/// Generate AOF entry for INCR command
///
/// # Arguments
/// * `k` - Key bytes to increment
pub fn emit_aof_incr(k: &[u8]) -> Vec<u8> {
    encode(b"INCR", [k].into_iter())
}

use bytes::Bytes;

/// Generate AOF entry for MSET command
///
/// Handles multiple key-value pairs in a single command.
///
/// # Arguments
/// * `pairs` - (key, value) pairs
pub fn emit_aof_mset(pairs: &[(Bytes, Bytes)]) -> Vec<u8> {
    encode(b"MSET", pairs.iter().flat_map(|(k, v)| [&k[..], &v[..]]))
}

/// Generate AOF entry for DEL command
///
/// # Arguments
/// * `keys` - Keys that were removed
pub fn emit_aof_del(keys: &[Bytes]) -> Vec<u8> {
    encode(b"DEL", keys.iter().map(|k| &k[..]))
}
