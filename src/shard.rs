/*!
 * Command Execution Shard
 *
 * This module implements the core command execution logic for Ignix.
 * A shard represents a single execution unit that processes Redis commands
 * and maintains its own storage and AOF logging.
 */

use crate::aof::{emit_aof_incr, emit_aof_mset, emit_aof_rename, emit_aof_set, AofHandle};
use crate::protocol::{
    parse_canonical_i64, write_array_len, write_bulk, write_error, write_integer, write_null,
    write_simple, Cmd, Value,
};
use crate::storage::Dict;
use bytes::{Bytes, BytesMut};

/// Choose how to store a value written by SET or MSET.
///
/// Canonical integers (the strings Redis would store with its integer
/// encoding) become `Value::Int`, which INCR can update in place and which
/// formats back to exactly the same bytes. Everything else, including
/// "007", "-0" or "+1", is stored byte for byte.
fn encode_value(v: Bytes) -> Value {
    match parse_canonical_i64(&v) {
        Some(i) => Value::Int(i),
        None => Value::Str(v),
    }
}

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
            // PING [message] - connectivity test; echoes the message when given
            Cmd::Ping(None) => write_simple("PONG", out),
            Cmd::Ping(Some(message)) => write_bulk(&message, out),

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

                self.dict.set(k, encode_value(v));

                write_simple("OK", out);
            }

            // DEL key [key ...] - delete keys, reply with the number removed
            Cmd::Del(mut keys) => {
                // Keep only the keys that were removed; a repeated key is
                // removed (and counted) once, like in Redis.
                keys.retain(|k| self.dict.del(k));
                write_integer(keys.len() as i64, out);
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

            // EXISTS key [key ...] - count existing keys; repeated keys count
            // every time, like in Redis
            Cmd::Exists(keys) => {
                let existing = keys.iter().filter(|k| self.dict.exists(k)).count();
                write_integer(existing as i64, out);
            }

            // INCR key - increment numeric value
            Cmd::Incr(k) => {
                // Keep the key for the AOF record only when persistence is on.
                let aof_key = self.aof.is_some().then(|| k.clone());
                match self.dict.incr(k) {
                    Ok(v) => {
                        // Log only successful increments
                        if let (Some(a), Some(key)) = (&self.aof, aof_key) {
                            a.write(&emit_aof_incr(&key));
                        }
                        write_integer(v, out);
                    }
                    Err(e) => write_error(e.as_str(), out),
                }
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
                    self.dict.set(k, encode_value(v));
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
        assert_eq!(
            std::mem::align_of::<Shard>(),
            64,
            "Shard struct should be aligned to 64 bytes"
        );
    }
}
