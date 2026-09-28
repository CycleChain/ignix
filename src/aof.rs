/*!
 * Append-Only File (AOF) Persistence
 *
 * This module implements Redis-compatible AOF persistence for durability.
 * Commands are logged in RESP format to a file and periodically flushed
 * to disk for crash recovery.
 */

use crate::protocol::{fmt_i64, fmt_u64};
use anyhow::*;
use crossbeam::channel::{bounded, RecvTimeoutError, Sender};
use std::io::Write;
use std::result::Result::{Err, Ok};
use std::time::{Duration, Instant};

/// How often written records are synced to disk
const SYNC_INTERVAL: Duration = Duration::from_secs(1);

/// How long the writer may block waiting for the next record: until the next
/// sync is due while written records are unsynced, otherwise indefinitely.
fn receive_timeout(unsynced: bool, since_sync: Duration) -> Option<Duration> {
    unsynced.then(|| SYNC_INTERVAL.saturating_sub(since_sync))
}

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
/// * An error if the file cannot be opened; the caller decides whether to
///   run without persistence
///
/// # Behavior
/// * Each record is written to the file as soon as it is received
/// * Written data is synced to disk about one second after the previous sync
///   at the latest, so no record stays unsynced for more than a second, also
///   when no more writes arrive
/// * Thread continues until every handle is dropped, then syncs and exits
pub fn spawn_aof_writer(path: &str) -> Result<AofHandle> {
    // Open the file here, so a path that cannot be used is reported to the
    // caller instead of panicking in the writer thread (which, with
    // `panic = "abort"`, would terminate the whole server).
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("cannot open AOF file {path}"))?;

    // Bounded channel to provide backpressure under heavy write load
    let (tx, rx) = bounded::<Vec<u8>>(4096);

    // Spawn dedicated AOF writer thread
    std::thread::Builder::new()
        .name("aof-writer".into())
        .spawn(move || {
            let mut last_sync = Instant::now();
            let mut unsynced = false;
            let mut write_failing = false;

            loop {
                // Wake up when the next sync is due, not a full interval after
                // the last record: that left records unsynced for up to two
                // intervals.
                let received = match receive_timeout(unsynced, last_sync.elapsed()) {
                    Some(timeout) => rx.recv_timeout(timeout),
                    None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                match received {
                    Ok(buf) => match file.write_all(&buf) {
                        Ok(()) => {
                            unsynced = true;
                            write_failing = false;
                        }
                        Err(e) => {
                            if !write_failing {
                                log::error!("AOF write failed, records are being lost: {e}");
                            }
                            write_failing = true;
                        }
                    },
                    Err(RecvTimeoutError::Timeout) => {}
                    // Every handle is gone: sync what was written and exit
                    Err(RecvTimeoutError::Disconnected) => {
                        if unsynced {
                            let _ = file.sync_data();
                        }
                        break;
                    }
                }

                if unsynced && last_sync.elapsed() >= SYNC_INTERVAL {
                    if let Err(e) = file.sync_data() {
                        log::error!("AOF sync failed: {e}");
                    }
                    unsynced = false;
                    last_sync = Instant::now();
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
/// Format: `*3\r\n$3\r\nSET\r\n$<keylen>\r\n<key>\r\n$<vallen>\r\n<val>\r\n`
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

/// Generate AOF entry for INCRBY command (also used for DECRBY and DECR)
///
/// # Arguments
/// * `k` - Key bytes
/// * `delta` - Amount added to the value
pub fn emit_aof_incrby(k: &[u8], delta: i64) -> Vec<u8> {
    let mut digits = [0u8; 20];
    encode(b"INCRBY", [k, fmt_i64(delta, &mut digits)].into_iter())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_wakes_up_when_the_next_sync_is_due() {
        // Nothing to sync: wait for the next record however long it takes
        assert_eq!(receive_timeout(false, Duration::from_millis(300)), None);
        // Unsynced data: wake up one interval after the previous sync
        assert_eq!(receive_timeout(true, Duration::ZERO), Some(SYNC_INTERVAL));
        assert_eq!(
            receive_timeout(true, Duration::from_millis(300)),
            Some(Duration::from_millis(700))
        );
        // Overdue: do not wait at all
        assert_eq!(
            receive_timeout(true, Duration::from_secs(5)),
            Some(Duration::ZERO)
        );
    }
}
