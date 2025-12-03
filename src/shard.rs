/*!
 * Command Execution Shard
 * 
 * This module implements the core command execution logic for Ignix.
 * A shard represents a single execution unit that processes Redis commands
 * and maintains its own storage and AOF logging.
 */

use crate::aof::{emit_aof_incr, emit_aof_mset, emit_aof_rename, emit_aof_set, AofHandle};
use crate::protocol::{write_array_len, write_bulk, write_integer, write_null, write_simple, Cmd, Value};
use crate::storage::Dict;
use bytes::BytesMut;

/// A shard represents a single execution unit
/// 
/// Each shard has its own storage dictionary and optional AOF handle
/// for persistence. In the current implementation, Ignix uses a single
/// shard, but the architecture supports multiple shards for future scaling.
#[repr(align(64))]
pub struct Shard {
    /// Unique identifier for this shard
    pub id: usize,
    /// In-memory storage dictionary
    pub dict: Dict,
    /// Optional AOF handle for persistence
    pub aof: Option<AofHandle>,
}

impl Shard {
    /// Create a new shard with the given ID and optional AOF handle
    /// 
    /// # Arguments
    /// * `id` - Unique identifier for this shard
    /// * `aof` - Optional AOF handle for command logging
    pub fn new(id: usize, aof: Option<AofHandle>) -> Self {
        Self {
            id,
            dict: Dict::default(),
            aof,
        }
    }
    
    /// Execute a Redis command and return the RESP-formatted response
    /// 
    /// This is the main entry point for command execution. It handles
    /// all supported Redis commands, updates the storage, logs to AOF
    /// if enabled, and returns the appropriate RESP response.
    /// 
    /// # Arguments
    /// * `cmd` - Parsed Redis command to execute
    /// 
    /// # Returns
    /// * RESP-formatted response as byte vector
    /// Execute a Redis command and write response directly to buffer
    /// 
    /// This is the main entry point for command execution. It handles
    /// all supported Redis commands, updates the storage, logs to AOF
    /// if enabled, and writes the RESP response directly to the output buffer.
    /// 
    /// # Arguments
    /// * `cmd` - Parsed Redis command to execute
    /// * `out` - Buffer to write response to
    pub fn exec(&self, cmd: Cmd, out: &mut BytesMut) {
        match cmd {
            // PING command - simple connectivity test
            Cmd::Ping => write_simple("PONG", out),
            
            // GET key - retrieve value for key
            Cmd::Get(k) => match self.dict.get(&k) {
                // Return string/blob values as bulk strings
                Some(Value::Str(v)) | Some(Value::Blob(v)) => write_bulk(&v, out),
                // Return integer values as Bulk Strings (Redis protocol requirement for GET)
                Some(Value::Int(i)) => write_bulk(i.to_string().as_bytes(), out),
                // Return null if key doesn't exist
                None => write_null(out),
            },
            
            // SET key value - store key-value pair
            Cmd::Set(k, v) => {
                // Log to AOF if persistence is enabled
                // We do this before moving k and v into the dictionary
                if let Some(a) = &self.aof {
                    a.write(&emit_aof_set(&k, &v));
                }

                // Optimization: Try to store as integer if possible
                // Fast fail: Integers fit in 20 chars and start with digit or '-'
                let val = if v.len() <= 20 && !v.is_empty() && (v[0].is_ascii_digit() || v[0] == b'-') {
                     if let Ok(s) = std::str::from_utf8(&v) {
                        if let Ok(i) = s.parse::<i64>() {
                            Value::Int(i)
                        } else {
                            Value::Str(v)
                        }
                    } else {
                        Value::Str(v)
                    }
                } else {
                    Value::Str(v)
                };

                self.dict.set(k, val);
                
                write_simple("OK", out);
            }
            
            // DEL key - delete key
            Cmd::Del(k) => {
                // Delete key and return 1 if it existed, 0 if not
                let removed = self.dict.del(&k) as i64;
                write_integer(removed, out);
            }
            
            // RENAME oldkey newkey - rename a key
            Cmd::Rename(from, to) => {
                if self.aof.is_some() {
                    let ok = self.dict.rename(from.clone(), to.clone());
                    if ok {
                         if let Some(a) = &self.aof {
                            a.write(&emit_aof_rename(&from, &to));
                        }
                        write_simple("OK", out);
                    } else {
                        write_simple("ERR no such key", out);
                    }
                } else {
                    // No AOF, we can move directly
                    let ok = self.dict.rename(from, to);
                    if ok {
                        write_simple("OK", out);
                    } else {
                        write_simple("ERR no such key", out);
                    }
                }
            }
            
            // EXISTS key - check if key exists
            Cmd::Exists(k) => write_integer(self.dict.exists(&k) as i64, out),
            
            // INCR key - increment numeric value
            Cmd::Incr(k) => {
                let v = self.dict.incr(&k);
                
                // Log increment to AOF
                if let Some(a) = &self.aof {
                    a.write(&emit_aof_incr(&k));
                }
                
                write_integer(v, out);
            }
            
            // MGET key1 key2 ... - get multiple keys
            Cmd::MGet(keys) => {
                write_array_len(keys.len(), out);
                
                // Get each key and format as RESP
                for k in keys {
                    match self.dict.get(&k) {
                        Some(Value::Str(v)) | Some(Value::Blob(v)) => write_bulk(&v, out),
                        Some(Value::Int(i)) => write_bulk(i.to_string().as_bytes(), out),
                        None => write_null(out),
                    }
                }
            }
            
            // MSET key1 value1 key2 value2 ... - set multiple key-value pairs
            Cmd::MSet(pairs) => {
                // Log all sets to AOF as a single operation
                if let Some(a) = &self.aof {
                    a.write(&emit_aof_mset(&pairs));
                }

                // Set all key-value pairs
                for (k, v) in pairs {
                    // Optimization: Try to store as integer if possible
                    // Fast fail: Integers fit in 20 chars and start with digit or '-'
                    let val = if v.len() <= 20 && !v.is_empty() && (v[0].is_ascii_digit() || v[0] == b'-') {
                        if let Ok(s) = std::str::from_utf8(&v) {
                            if let Ok(i) = s.parse::<i64>() {
                                Value::Int(i)
                            } else {
                                Value::Str(v)
                            }
                        } else {
                            Value::Str(v)
                        }
                    } else {
                        Value::Str(v)
                    };
                    self.dict.set(k, val);
                }
                
                write_simple("OK", out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shard_alignment() {
        assert_eq!(std::mem::align_of::<Shard>(), 64, "Shard struct should be aligned to 64 bytes");
    }
}