/*!
 * Command Execution Shard
 *
 * This module implements the core command execution logic for Ignix.
 * A shard represents a single execution unit that processes Redis commands
 * and maintains its own storage and AOF logging.
 */

use crate::aof::{
    emit_aof_del, emit_aof_incr, emit_aof_incrby, emit_aof_mset, emit_aof_rename, emit_aof_set,
    AofHandle,
};
use crate::protocol::{
    fmt_i64, parse_canonical_i64, write_array_len, write_bulk, write_error, write_integer,
    write_null, write_simple, Cmd, Value,
};
use crate::session::Session;
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

/// Values up to this size are copied into a GET reply while the shard's read
/// lock is held; larger ones are cloned (a reference count) and written after
/// the lock is released, so a long copy never holds up writers.
const COPY_UNDER_LOCK_MAX: usize = 16 * 1024;

/// Write a stored value, or null when the key is missing, as a GET reply.
///
/// Integers are sent as bulk strings, as Redis does for GET.
fn write_value(value: Option<&Value>, out: &mut BytesMut) {
    match value {
        Some(Value::Str(v)) | Some(Value::Blob(v)) => write_bulk(v, out),
        Some(Value::Int(i)) => {
            let mut digits = [0u8; 20];
            write_bulk(fmt_i64(*i, &mut digits), out);
        }
        None => write_null(out),
    }
}

/// Write the GET reply for `key`.
///
/// Small values are copied straight from the dictionary, which avoids the
/// two atomic reference-count updates of cloning `Bytes` (contended when
/// many connections read the same key).
fn write_get(dict: &Dict, key: &[u8], out: &mut BytesMut) {
    let large = dict.read(key, |value| match value {
        Some(Value::Str(v)) | Some(Value::Blob(v)) if v.len() > COPY_UNDER_LOCK_MAX => {
            Some(v.clone())
        }
        small => {
            write_value(small, out);
            None
        }
    });
    if let Some(v) = large {
        write_bulk(&v, out);
    }
}

/// Write the MGET reply values for `keys`, all read while their shards are
/// locked together.
///
/// Values are copied like in [`write_get`] up to the first large one; from
/// there on they are cloned and written after the locks are released.
fn write_mget(dict: &Dict, keys: &[Bytes], out: &mut BytesMut) {
    let mut later = Vec::new();
    dict.read_many(keys, |value| {
        let large = matches!(
            value,
            Some(Value::Str(v)) | Some(Value::Blob(v)) if v.len() > COPY_UNDER_LOCK_MAX
        );
        if large || !later.is_empty() {
            later.push(value.cloned());
        } else {
            write_value(value, out);
        }
    });
    for value in &later {
        write_value(value.as_ref(), out);
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
    /// Runs the command like [`Shard::exec_session`] on a fresh session,
    /// so commands that change the connection (`QUIT`) only reply.
    ///
    /// # Arguments
    /// * `cmd` - Parsed Redis command to execute
    /// * `out` - Buffer to write response to
    pub fn exec(&self, cmd: Cmd, out: &mut BytesMut) {
        self.exec_session(cmd, &mut Session::default(), out)
    }

    /// Execute a command for the connection `session` belongs to
    ///
    /// This is the main entry point for command execution. It handles
    /// all supported Redis commands, updates the storage and the session,
    /// logs to AOF if enabled, and writes the RESP response directly to the
    /// output buffer. After `QUIT`, [`Session::is_closing`] is true: the
    /// server must send the replies and close the connection without
    /// running any later request.
    pub fn exec_session(&self, cmd: Cmd, session: &mut Session, out: &mut BytesMut) {
        match cmd {
            // PING [message] - connectivity test; echoes the message when given
            Cmd::Ping(None) => write_simple("PONG", out),
            Cmd::Ping(Some(message)) => write_bulk(&message, out),

            // GET key - retrieve value for key
            Cmd::Get(k) => write_get(&self.dict, &k, out),

            // SET key value - store key-value pair
            Cmd::Set(k, v) => {
                // Log to AOF if persistence is enabled
                // We do this before moving k and v into the dictionary
                if let Some(a) = &self.aof {
                    a.write_owned(emit_aof_set(&k, &v));
                }

                self.dict.set(k, encode_value(v));

                write_simple("OK", out);
            }

            // DEL key [key ...] - delete keys, reply with the number removed
            Cmd::Del(mut keys) => {
                // Keep only the keys that were removed; a repeated key is
                // removed (and counted) once, like in Redis.
                self.dict.del_many(&mut keys);
                if let Some(a) = &self.aof {
                    if !keys.is_empty() {
                        a.write_owned(emit_aof_del(&keys));
                    }
                }
                write_integer(keys.len() as i64, out);
            }

            // RENAME oldkey newkey - rename a key
            Cmd::Rename(from, to) => {
                // Encode the AOF record before the keys move into the map;
                // it is only written if the rename succeeds.
                let record = self.aof.as_ref().map(|_| emit_aof_rename(&from, &to));
                if self.dict.rename(from, to) {
                    if let (Some(a), Some(record)) = (&self.aof, record) {
                        a.write_owned(record);
                    }
                    write_simple("OK", out);
                } else {
                    write_error("ERR no such key", out);
                }
            }

            // EXISTS key [key ...] - count existing keys; repeated keys count
            // every time, like in Redis
            Cmd::Exists(keys) => {
                let mut existing = 0;
                self.dict
                    .read_many(&keys, |value| existing += value.is_some() as i64);
                write_integer(existing, out);
            }

            // INCR key - increment numeric value
            Cmd::Incr(k) => {
                // Keep the key for the AOF record only when persistence is on.
                let aof_key = self.aof.is_some().then(|| k.clone());
                match self.dict.incr(k) {
                    Ok(v) => {
                        // Log only successful increments
                        if let (Some(a), Some(key)) = (&self.aof, aof_key) {
                            a.write_owned(emit_aof_incr(&key));
                        }
                        write_integer(v, out);
                    }
                    Err(e) => write_error(e.as_str(), out),
                }
            }

            // INCRBY / DECRBY / DECR - add a delta to a numeric value
            Cmd::IncrBy(k, delta) => {
                let aof_key = self.aof.is_some().then(|| k.clone());
                match self.dict.incr_by(k, delta) {
                    Ok(v) => {
                        if let (Some(a), Some(key)) = (&self.aof, aof_key) {
                            a.write_owned(emit_aof_incrby(&key, delta));
                        }
                        write_integer(v, out);
                    }
                    Err(e) => write_error(e.as_str(), out),
                }
            }

            // MGET key1 key2 ... - get multiple keys
            Cmd::MGet(keys) => {
                write_array_len(keys.len(), out);
                write_mget(&self.dict, &keys, out);
            }

            // MSET key1 value1 key2 value2 ... - set multiple key-value pairs
            Cmd::MSet(pairs) => {
                // Log all sets to AOF as a single operation
                if let Some(a) = &self.aof {
                    a.write_owned(emit_aof_mset(&pairs));
                }

                // Set all key-value pairs at once
                self.dict.set_many(pairs, encode_value);

                write_simple("OK", out);
            }

            // ECHO message
            Cmd::Echo(message) => write_bulk(&message, out),

            // QUIT - the connection closes once this reply is sent
            Cmd::Quit => {
                write_simple("OK", out);
                session.close();
            }

            // SELECT index - there is a single database, number 0
            Cmd::Select(0) => write_simple("OK", out),
            Cmd::Select(_) => write_error("ERR DB index is out of range", out),
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
