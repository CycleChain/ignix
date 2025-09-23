/*!
 * Command Execution Shard
 * 
 * This module implements the core command execution logic for Ignix.
 * A shard represents a single execution unit that processes Redis commands
 * and maintains its own storage and AOF logging.
 */

use crate::aof::{emit_aof_incr, emit_aof_mset, emit_aof_rename, emit_aof_set, AofHandle};
use crate::protocol::{resp_array, resp_bulk, resp_integer, resp_null, resp_simple, Cmd, Value};
use crate::storage::Dict;

/// A shard represents a single execution unit
/// 
/// Each shard has its own storage dictionary and optional AOF handle
/// for persistence. In the current implementation, Ignix uses a single
/// shard, but the architecture supports multiple shards for future scaling.
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
    pub fn exec(&mut self, cmd: Cmd) -> Vec<u8> {
        match cmd {
            // PING command - simple connectivity test
            Cmd::Ping => resp_simple("PONG"),
            
            // GET key - retrieve value for key
            Cmd::Get(k) => match self.dict.get(&k) {
                // Return string/blob values as bulk strings
                Some(Value::Str(v)) | Some(Value::Blob(v)) => resp_bulk(v),
                // Return integer values as RESP integers
                Some(Value::Int(i)) => resp_integer(*i),
                // Return null if key doesn't exist
                None => resp_null(),
            },
            
            // SET key value - store key-value pair
            Cmd::Set(k, v) => {
                // Store the value as a string
                self.dict.set(k.clone(), Value::Str(v.clone()));
                
                // Log to AOF if persistence is enabled
                if let Some(a) = &self.aof {
                    a.write(&emit_aof_set(&k, &v));
                }
                
                resp_simple("OK")
            }
            
            // DEL key - delete key
            Cmd::Del(k) => {
                // Delete key and return 1 if it existed, 0 if not
                let removed = self.dict.del(&k) as i64;
                resp_integer(removed)
            }
            
            // RENAME oldkey newkey - rename a key
            Cmd::Rename(from, to) => {
                let ok = self.dict.rename(from.clone(), to.clone());
                if ok {
                    // Log successful rename to AOF
                    if let Some(a) = &self.aof {
                        a.write(&emit_aof_rename(&from, &to));
                    }
                    resp_simple("OK")
                } else {
                    // Return error if source key doesn't exist
                    resp_simple("ERR no such key")
                }
            }
            
            // EXISTS key - check if key exists
            Cmd::Exists(k) => resp_integer(self.dict.exists(&k) as i64),
            
            // INCR key - increment numeric value
            Cmd::Incr(k) => {
                let v = match self.dict.get_mut(&k) {
                    // If key exists and is an integer, increment it
                    Some(Value::Int(i)) => {
                        *i += 1;
                        *i
                    }
                    // If key exists and is a string, try to parse as number
                    Some(Value::Str(s)) => {
                        let mut n = std::str::from_utf8(s)
                            .ok()
                            .and_then(|x| x.parse::<i64>().ok())
                            .unwrap_or(0);
                        n += 1;
                        // Update the string representation
                        *s = n.to_string().into_bytes();
                        n
                    }
                    // If key exists but is not a valid type, return 0
                    Some(_) => 0,
                    // If key doesn't exist, create it with value 1
                    None => {
                        self.dict.set(k.clone(), Value::Int(1));
                        1
                    }
                };
                
                // Log increment to AOF
                if let Some(a) = &self.aof {
                    a.write(&emit_aof_incr(&k));
                }
                
                resp_integer(v)
            }
            
            // MGET key1 key2 ... - get multiple keys
            Cmd::MGet(keys) => {
                // Pre-allocate vector for better performance
                let mut items = Vec::with_capacity(keys.len());
                
                // Get each key and format as RESP
                for k in keys {
                    let b = match self.dict.get(&k) {
                        Some(Value::Str(v)) | Some(Value::Blob(v)) => resp_bulk(v),
                        Some(Value::Int(i)) => resp_integer(*i),
                        None => resp_null(),
                    };
                    items.push(b);
                }
                
                // Return as RESP array
                resp_array(items)
            }
            
            // MSET key1 value1 key2 value2 ... - set multiple key-value pairs
            Cmd::MSet(pairs) => {
                // Set all key-value pairs
                for (k, v) in pairs.iter() {
                    self.dict.set(k.clone(), Value::Str(v.clone()));
                }
                
                // Log all sets to AOF as a single operation
                if let Some(a) = &self.aof {
                    a.write(&emit_aof_mset(&pairs));
                }
                
                resp_simple("OK")
            }
        }
    }
}