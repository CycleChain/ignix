/*!
 * Command Execution Shard
 *
 * This module implements the core command execution logic for Ignix.
 * A shard represents a single execution unit that processes Redis commands
 * and maintains its own storage and AOF logging.
 */

use crate::aof::{
    emit_aof_del, emit_aof_flushall, emit_aof_flushdb, emit_aof_incr, emit_aof_incrby,
    emit_aof_mset, emit_aof_rename, emit_aof_set, AofHandle,
};
use crate::glob::Pattern;
use crate::info::{write_config_get, write_info, REDIS_VERSION};
use crate::protocol::{
    fmt_i64, fmt_u64, parse_canonical_i64, write_array_len, write_bulk, write_error, write_integer,
    write_map_len, write_nil, write_simple, Cmd, FlushMode, Protocol, Value,
};
use crate::session::Session;
use crate::stats::Stats;
use crate::storage::Dict;
use bytes::{Bytes, BytesMut};
use std::sync::Arc;

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
fn write_value(value: Option<&Value>, protocol: Protocol, out: &mut BytesMut) {
    match value {
        Some(Value::Str(v)) | Some(Value::Blob(v)) => write_bulk(v, out),
        Some(Value::Int(i)) => {
            let mut digits = [0u8; 20];
            write_bulk(fmt_i64(*i, &mut digits), out);
        }
        None => write_nil(protocol, out),
    }
}

/// Write the GET reply for `key`.
///
/// Small values are copied straight from the dictionary, which avoids the
/// two atomic reference-count updates of cloning `Bytes` (contended when
/// many connections read the same key).
fn write_get(dict: &Dict, key: &[u8], protocol: Protocol, out: &mut BytesMut) {
    let large = dict.read(key, |value| match value {
        Some(Value::Str(v)) | Some(Value::Blob(v)) if v.len() > COPY_UNDER_LOCK_MAX => {
            Some(v.clone())
        }
        small => {
            write_value(small, protocol, out);
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
fn write_mget(dict: &Dict, keys: &[Bytes], protocol: Protocol, out: &mut BytesMut) {
    let mut later = Vec::new();
    dict.read_many(keys, |value| {
        let large = matches!(
            value,
            Some(Value::Str(v)) | Some(Value::Blob(v)) if v.len() > COPY_UNDER_LOCK_MAX
        );
        if large || !later.is_empty() {
            later.push(value.cloned());
        } else {
            write_value(value, protocol, out);
        }
    });
    for value in &later {
        write_value(value.as_ref(), protocol, out);
    }
}

/// Compile a KEYS or SCAN pattern; `*` alone needs no matching (and, unlike
/// other patterns, also matches the empty key, as in Redis)
fn glob(pattern: &[u8]) -> Option<Pattern> {
    (pattern != b"*").then(|| Pattern::new(pattern, false))
}

/// `CONFIG HELP`, in Redis's words, for the subcommands Ignix supports
const CONFIG_HELP: [&str; 5] = [
    "CONFIG <subcommand> [<arg> [value] [opt] ...]. Subcommands are:",
    "GET <pattern>",
    "    Return parameters matching the glob-like <pattern> and their values.",
    "HELP",
    "    Prints this help.",
];

/// Write a HELP reply: one status line per line of text
fn write_help(lines: &[&str], out: &mut BytesMut) {
    write_array_len(lines.len(), out);
    for line in lines {
        write_simple(line, out);
    }
}

/// `CLIENT HELP`, in Redis's words, for the subcommands Ignix supports
const CLIENT_HELP: [&str; 13] = [
    "CLIENT <subcommand> [<arg> [value] [opt] ...]. Subcommands are:",
    "GETNAME",
    "    Return the name of the current connection.",
    "ID",
    "    Return the ID of the current connection.",
    "SETINFO <option> <value>",
    "    Set client meta attr. Options are:",
    "    * LIB-NAME: the client lib name.",
    "    * LIB-VER: the client lib version.",
    "SETNAME <name>",
    "    Assign the name <name> to the current connection.",
    "HELP",
    "    Prints this help.",
];

/// Run HELLO's options in order, stopping at the first that fails (Redis
/// applies each one as it goes), then switch to `protocol` and describe the
/// server.
fn hello(protocol: Option<Protocol>, options: &[Bytes], session: &mut Session, out: &mut BytesMut) {
    let mut i = 0;
    while i < options.len() {
        let option = &options[i];
        let more = options.len() - 1 - i;
        if option.eq_ignore_ascii_case(b"AUTH") && more >= 2 {
            // No password is set, so only the default user exists and it
            // accepts any password (Redis `nopass`).
            if options[i + 1] != b"default"[..] {
                write_error(
                    "WRONGPASS invalid username-password pair or user is disabled.",
                    out,
                );
                return;
            }
            i += 3;
        } else if option.eq_ignore_ascii_case(b"SETNAME") && more >= 1 {
            if let Err(error) = session.set_name(options[i + 1].clone()) {
                write_error(error, out);
                return;
            }
            i += 2;
        } else {
            let option = String::from_utf8_lossy(option);
            write_error(&format!("ERR Syntax error in HELLO option '{option}'"), out);
            return;
        }
    }
    if let Some(protocol) = protocol {
        session.set_protocol(protocol);
    }

    let protocol = session.protocol();
    write_map_len(protocol, 7, out);
    for (field, value) in [("server", "redis"), ("version", REDIS_VERSION)] {
        write_bulk(field.as_bytes(), out);
        write_bulk(value.as_bytes(), out);
    }
    write_bulk(b"proto", out);
    write_integer(if protocol == Protocol::Resp3 { 3 } else { 2 }, out);
    write_bulk(b"id", out);
    write_integer(session.id() as i64, out);
    for (field, value) in [("mode", "standalone"), ("role", "master")] {
        write_bulk(field.as_bytes(), out);
        write_bulk(value.as_bytes(), out);
    }
    write_bulk(b"modules", out);
    write_array_len(0, out);
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
    /// Counters reported by INFO
    pub stats: Arc<Stats>,
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
            stats: Arc::default(),
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

    /// Remove every key, logging `record()` while the keys are locked so
    /// that it keeps its place among the other writes
    fn flush(&self, mode: FlushMode, record: fn() -> Vec<u8>, out: &mut BytesMut) {
        self.dict.flush(mode == FlushMode::Async, || {
            if let Some(a) = &self.aof {
                a.write_owned(record());
            }
        });
        write_simple("OK", out);
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
        session.count_command();
        match cmd {
            // PING [message] - connectivity test; echoes the message when given
            Cmd::Ping(None) => write_simple("PONG", out),
            Cmd::Ping(Some(message)) => write_bulk(&message, out),

            // GET key - retrieve value for key
            Cmd::Get(k) => write_get(&self.dict, &k, session.protocol(), out),

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

            // DEL / UNLINK key [key ...] - delete keys, reply with the number
            // removed (UNLINK is logged as DEL)
            Cmd::Del(mut keys) | Cmd::Unlink(mut keys) => {
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
                write_mget(&self.dict, &keys, session.protocol(), out);
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

            // HELLO [protover [AUTH username password] [SETNAME clientname]]
            Cmd::Hello { protocol, options } => hello(protocol, &options, session, out),

            // DBSIZE - number of keys
            Cmd::DbSize => write_integer(self.dict.len() as i64, out),

            // TYPE key - every stored value is a string
            Cmd::Type(key) => {
                let exists = self.dict.read(&key, |value| value.is_some());
                write_simple(if exists { "string" } else { "none" }, out);
            }

            // KEYS pattern
            Cmd::Keys(pattern) => {
                let keys = self.dict.keys(glob(&pattern).as_ref());
                write_array_len(keys.len(), out);
                for key in &keys {
                    write_bulk(key, out);
                }
            }

            // SCAN cursor [MATCH pattern] [COUNT count] [TYPE type]
            Cmd::Scan {
                cursor,
                pattern,
                count,
                type_name,
            } => {
                let pattern = pattern.and_then(|p| glob(&p));
                let (next, mut keys) = self.dict.scan(cursor, count, pattern.as_ref());
                // Every value is a string
                if type_name.is_some_and(|t| !t.eq_ignore_ascii_case(b"string")) {
                    keys.clear();
                }
                write_array_len(2, out);
                let mut digits = [0u8; 20];
                write_bulk(fmt_u64(next, &mut digits), out);
                write_array_len(keys.len(), out);
                for key in &keys {
                    write_bulk(key, out);
                }
            }

            // FLUSHDB / FLUSHALL [ASYNC|SYNC] - there is one database, so both
            // remove every key
            Cmd::FlushDb(mode) => self.flush(mode, emit_aof_flushdb, out),
            Cmd::FlushAll(mode) => self.flush(mode, emit_aof_flushall, out),

            // CLIENT subcommands about the current connection
            Cmd::ClientId => write_integer(session.id() as i64, out),
            Cmd::ClientGetName => match session.name() {
                Some(name) => write_bulk(name, out),
                None => write_nil(session.protocol(), out),
            },
            Cmd::ClientSetName(name) => match session.set_name(name) {
                Ok(()) => write_simple("OK", out),
                Err(error) => write_error(error, out),
            },
            Cmd::ClientSetInfo(info, value) => {
                session.set_info(info, value);
                write_simple("OK", out);
            }
            Cmd::ClientHelp => write_help(&CLIENT_HELP, out),

            // INFO [section ...]
            Cmd::Info(sections) => write_info(self, &sections, session.protocol(), out),

            // CONFIG GET parameter [parameter ...] / CONFIG HELP
            Cmd::ConfigGet(patterns) => write_config_get(self, &patterns, session.protocol(), out),
            Cmd::ConfigHelp => write_help(&CONFIG_HELP, out),
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
