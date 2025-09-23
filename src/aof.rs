/*!
 * Append-Only File (AOF) Persistence
 * 
 * This module implements Redis-compatible AOF persistence for durability.
 * Commands are logged in RESP format to a file and periodically flushed
 * to disk for crash recovery.
 */

use anyhow::*;
use crossbeam::channel::{unbounded, Sender};
use std::io::Write;
use std::time::{Duration, Instant};
use std::result::Result::{Ok, Err};

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
    let (tx, rx) = unbounded::<Vec<u8>>();
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
                        // Write command to file (may be buffered by OS)
                        let _ = f.write_all(&buf);
                        
                        // Flush and sync to disk every second for durability
                        if last.elapsed() >= Duration::from_millis(1000) {
                            let _ = f.flush();     // Flush to OS buffers
                            let _ = f.sync_data(); // Force write to disk
                            last = Instant::now();
                        }
                    }
                    // Channel closed, exit thread
                    Err(_) => break,
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
// These functions generate RESP-formatted commands for logging to AOF.
// The format is human-readable and compatible with Redis AOF files.
//

/// Generate AOF entry for SET command
/// 
/// Creates a RESP-formatted SET command for AOF logging.
/// Format: *3\r\n$3\r\nSET\r\n$<keylen>\r\n<key>\r\n$<vallen>\r\n<val>\r\n
/// 
/// # Arguments
/// * `k` - Key bytes
/// * `v` - Value bytes
pub fn emit_aof_set(k: &[u8], v: &[u8]) -> Vec<u8> {
    format!(
        "*3\r\n$3\r\nSET\r\n${}\r\n{}\r\n${}\r\n{}\r\n",
        k.len(),
        String::from_utf8_lossy(k),
        v.len(),
        String::from_utf8_lossy(v)
    )
    .into_bytes()
}

/// Generate AOF entry for RENAME command
/// 
/// Creates a RESP-formatted RENAME command for AOF logging.
/// 
/// # Arguments
/// * `a` - Old key bytes
/// * `b` - New key bytes
pub fn emit_aof_rename(a: &[u8], b: &[u8]) -> Vec<u8> {
    format!(
        "*3\r\n$6\r\nRENAME\r\n${}\r\n{}\r\n${}\r\n{}\r\n",
        a.len(),
        String::from_utf8_lossy(a),
        b.len(),
        String::from_utf8_lossy(b)
    )
    .into_bytes()
}

/// Generate AOF entry for INCR command
/// 
/// Creates a RESP-formatted INCR command for AOF logging.
/// 
/// # Arguments
/// * `k` - Key bytes to increment
pub fn emit_aof_incr(k: &[u8]) -> Vec<u8> {
    format!(
        "*2\r\n$4\r\nINCR\r\n${}\r\n{}\r\n",
        k.len(),
        String::from_utf8_lossy(k)
    )
    .into_bytes()
}

/// Generate AOF entry for MSET command
/// 
/// Creates a RESP-formatted MSET command for AOF logging.
/// Handles multiple key-value pairs in a single command.
/// 
/// # Arguments
/// * `pairs` - Vector of (key, value) byte pairs
pub fn emit_aof_mset(pairs: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    // Calculate total arguments: command + (key + value) * pairs
    let mut s = format!("*{}\r\n$4\r\nMSET\r\n", 1 + pairs.len() * 2);
    
    // Add each key-value pair
    for (k, v) in pairs {
        s.push_str(&format!(
            "${}\r\n{}\r\n${}\r\n{}\r\n",
            k.len(),
            String::from_utf8_lossy(k),
            v.len(),
            String::from_utf8_lossy(v)
        ));
    }
    
    s.into_bytes()
}