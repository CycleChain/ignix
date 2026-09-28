/*!
 * Append-Only File (AOF) Persistence
 *
 * This module implements Redis-compatible AOF persistence for durability.
 * Commands are logged in RESP format to a file and periodically flushed
 * to disk for crash recovery.
 */

use crate::protocol::{fmt_i64, fmt_u64};
use anyhow::*;
use crossbeam::channel::{bounded, Receiver, RecvTimeoutError, Sender};
use std::fs::File;
use std::io::Write;
use std::result::Result::{Err, Ok};
use std::time::{Duration, Instant};

/// How often written records are synced to disk
const SYNC_INTERVAL: Duration = Duration::from_secs(1);

/// Most bytes of queued records collected into one write; a larger record is
/// written on its own
const MAX_BATCH: usize = 1 << 20;

/// Pause after each write, so that records queue up in the meantime and the
/// next write covers many of them. Senders only wake the writer when it is
/// blocked waiting for a record, and waking it for every record cost the
/// workers more CPU than the rest of a SET.
const GROUP_COMMIT_DELAY: Duration = Duration::from_micros(200);

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
/// * Records are appended in the order they were sent. The writer collects
///   the queued records into as few writes as possible and, once the queue
///   is empty, pauses for 200 µs so that the next write covers the records
///   sent meanwhile
/// * Written data is synced to disk about one second after the previous sync
///   at the latest, so no record stays unsynced for more than a second, also
///   when no more writes arrive
/// * Thread continues until every handle is dropped, then syncs and exits
pub fn spawn_aof_writer(path: &str) -> Result<AofHandle> {
    // Open the file here, so a path that cannot be used is reported to the
    // caller instead of panicking in the writer thread (which, with
    // `panic = "abort"`, would terminate the whole server).
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("cannot open AOF file {path}"))?;
    let len = file
        .metadata()
        .with_context(|| format!("cannot read the size of AOF file {path}"))?
        .len();

    // Bounded channel to provide backpressure under heavy write load
    let (tx, rx) = bounded::<Vec<u8>>(4096);

    // Spawn dedicated AOF writer thread
    std::thread::Builder::new()
        .name("aof-writer".into())
        .spawn(move || run_writer(AofFile::new(file, len), rx))?;

    Ok(AofHandle { tx })
}

/// Where the writer thread puts records: the AOF file, or a fake in tests
trait AofSink: Write {
    /// Cut the file back to `len` bytes
    fn truncate(&mut self, len: u64) -> std::io::Result<()>;
    /// Make the written data durable
    fn sync(&mut self) -> std::io::Result<()>;
}

impl AofSink for File {
    fn truncate(&mut self, len: u64) -> std::io::Result<()> {
        self.set_len(len)
    }

    fn sync(&mut self) -> std::io::Result<()> {
        self.sync_data()
    }
}

/// The AOF file with its sync and error state
struct AofFile<W> {
    file: W,
    /// File length up to the end of the last complete record
    len: u64,
    /// A failed write left part of a record after `len` that could not be
    /// removed yet; nothing may be appended after it
    torn: bool,
    unsynced: bool,
    last_sync: Instant,
    write_failing: bool,
}

impl<W: AofSink> AofFile<W> {
    fn new(file: W, len: u64) -> Self {
        Self {
            file,
            len,
            torn: false,
            unsynced: false,
            last_sync: Instant::now(),
            write_failing: false,
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // Never append after part of a record, or every later record would
        // be misread: cut it off first (as Redis does), or drop the records.
        if self.torn && self.file.truncate(self.len).is_ok() {
            self.torn = false;
        }
        let result = if self.torn {
            Err(std::io::Error::other(
                "the file ends with part of a record that cannot be removed",
            ))
        } else {
            self.file.write_all(bytes)
        };
        match result {
            Ok(()) => {
                self.len += bytes.len() as u64;
                self.unsynced = true;
                self.write_failing = false;
            }
            Err(e) => {
                // write_all may have written part of the bytes before failing
                if !self.torn {
                    self.torn = self.file.truncate(self.len).is_err();
                }
                // Log the first failure of a streak, not every record
                if !self.write_failing {
                    log::error!("AOF write failed, records are being lost: {e}");
                }
                self.write_failing = true;
            }
        }
    }

    fn sync(&mut self) {
        match self.file.sync() {
            Ok(()) => self.unsynced = false,
            // The data stays unsynced, so the next interval tries again
            Err(e) => log::error!("AOF sync failed, retrying in a second: {e}"),
        }
        self.last_sync = Instant::now();
    }
}

/// Body of the writer thread
fn run_writer<W: AofSink>(mut aof: AofFile<W>, rx: Receiver<Vec<u8>>) {
    let mut batch = Vec::with_capacity(64 * 1024);

    loop {
        // Wake up when the next sync is due, not a full interval after the
        // last record: that left records unsynced for up to two intervals.
        let received = match receive_timeout(aof.unsynced, aof.last_sync.elapsed()) {
            Some(timeout) => rx.recv_timeout(timeout),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let drained = match received {
            Ok(record) => write_queued(&mut aof, &mut batch, record, &rx),
            Err(RecvTimeoutError::Timeout) => false,
            // Every handle is gone and every record has been received: sync
            // what was written and exit
            Err(RecvTimeoutError::Disconnected) => {
                if aof.unsynced {
                    aof.sync();
                }
                return;
            }
        };

        if aof.unsynced && aof.last_sync.elapsed() >= SYNC_INTERVAL {
            aof.sync();
        }
        // Pause only after everything queued has been written: when the
        // batch limit cut a write short, keep draining at full speed.
        if drained {
            std::thread::sleep(GROUP_COMMIT_DELAY);
        }
    }
}

/// Write `first` and the records queued behind it, collected into as few
/// writes as possible (up to `MAX_BATCH` bytes each; a larger record is
/// written on its own, in order).
///
/// Returns whether the queue was drained, i.e. false when the batch limit
/// stopped the collection with records still queued.
fn write_queued<W: AofSink>(
    aof: &mut AofFile<W>,
    batch: &mut Vec<u8>,
    first: Vec<u8>,
    rx: &Receiver<Vec<u8>>,
) -> bool {
    let mut next = Some(first);
    let mut drained = false;
    while let Some(record) = next {
        if record.len() >= MAX_BATCH {
            // Keep the order without copying the large record
            aof.write(batch);
            batch.clear();
            aof.write(&record);
        } else {
            batch.extend_from_slice(&record);
        }
        next = if batch.len() < MAX_BATCH {
            let queued = rx.try_recv().ok();
            drained = queued.is_none();
            queued
        } else {
            None
        };
    }
    aof.write(batch);
    batch.clear();
    drained
}

impl AofHandle {
    /// Write a command to the AOF
    ///
    /// Copies `bytes` and sends them to the background writer thread; use
    /// [`AofHandle::write_owned`] to hand over an encoded record without the
    /// copy. Returns immediately unless the queue (4096 records) is full.
    ///
    /// # Arguments
    /// * `bytes` - RESP-formatted command bytes to write
    #[inline]
    pub fn write(&self, bytes: &[u8]) {
        self.write_owned(bytes.to_vec());
    }

    /// Send an encoded record, such as the result of [`emit_aof_set`], to the
    /// background writer thread without copying it
    ///
    /// Returns immediately unless the queue (4096 records) is full.
    #[inline]
    pub fn write_owned(&self, record: Vec<u8>) {
        // Send to background thread, ignore errors (channel closed)
        let _ = self.tx.send(record);
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

    /// In-memory sink that can fail writes after a byte limit, truncation and
    /// syncs
    #[derive(Default)]
    struct FakeSink {
        data: Vec<u8>,
        write_limit: Option<usize>,
        fail_truncate: bool,
        fail_sync: bool,
    }

    impl Write for FakeSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let room = match self.write_limit {
                Some(limit) => limit.saturating_sub(self.data.len()),
                None => buf.len(),
            };
            if room == 0 {
                return Err(std::io::Error::other("disk full"));
            }
            let n = room.min(buf.len());
            self.data.extend_from_slice(&buf[..n]);
            Ok(n)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl AofSink for FakeSink {
        fn truncate(&mut self, len: u64) -> std::io::Result<()> {
            if self.fail_truncate {
                return Err(std::io::Error::other("truncate failed"));
            }
            self.data.truncate(len as usize);
            Ok(())
        }

        fn sync(&mut self) -> std::io::Result<()> {
            if self.fail_sync {
                return Err(std::io::Error::other("sync failed"));
            }
            Ok(())
        }
    }

    #[test]
    fn failed_write_is_cut_off_before_the_next_record() {
        let mut aof = AofFile::new(FakeSink::default(), 0);
        aof.write(b"first");
        // The disk fills up in the middle of the next batch
        aof.file.write_limit = Some(8);
        aof.write(b"second");
        aof.file.write_limit = None;
        aof.write(b"third");
        assert_eq!(aof.file.data, b"firstthird");
    }

    #[test]
    fn nothing_is_appended_after_a_partial_record_that_cannot_be_removed() {
        let mut aof = AofFile::new(FakeSink::default(), 0);
        aof.write(b"first");
        aof.file.write_limit = Some(8);
        aof.file.fail_truncate = true;
        aof.write(b"second");
        aof.file.write_limit = None;
        aof.write(b"third");
        assert_eq!(aof.file.data, b"firstsec", "no record after a torn one");
        // Once the partial record can be removed, writing resumes
        aof.file.fail_truncate = false;
        aof.write(b"fourth");
        assert_eq!(aof.file.data, b"firstfourth");
    }

    #[test]
    fn writer_keeps_draining_while_the_batch_limit_leaves_records_queued() {
        let (tx, rx) = bounded::<Vec<u8>>(16);
        let mut aof = AofFile::new(FakeSink::default(), 0);
        let mut batch = Vec::new();

        // Three records of 600 KiB: the second one fills the 1 MiB batch
        for _ in 0..3 {
            tx.send(vec![b'x'; 600 << 10]).unwrap();
        }
        let first = rx.recv().unwrap();
        assert!(!write_queued(&mut aof, &mut batch, first, &rx));
        assert_eq!(aof.file.data.len(), 1200 << 10);
        assert_eq!(rx.len(), 1, "the record after the limit is still queued");

        // The last one drains the queue
        let first = rx.recv().unwrap();
        assert!(write_queued(&mut aof, &mut batch, first, &rx));
        assert_eq!(aof.file.data.len(), 1800 << 10);
    }

    #[test]
    fn failed_sync_is_retried() {
        let mut aof = AofFile::new(FakeSink::default(), 0);
        aof.write(b"record");
        aof.file.fail_sync = true;
        aof.sync();
        assert!(aof.unsynced, "a failed sync must not count as durable");
        aof.file.fail_sync = false;
        aof.sync();
        assert!(!aof.unsynced);
    }

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
