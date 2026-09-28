/*!
 * Command Execution Shard
 *
 * This module implements the core command execution logic for Ignix.
 * A shard represents a single execution unit that processes Redis commands
 * and maintains its own storage and AOF logging.
 */

use crate::aof::{
    emit_aof_del, emit_aof_flushall, emit_aof_flushdb, emit_aof_incr, emit_aof_incrby,
    emit_aof_mset, emit_aof_persist, emit_aof_pexpireat, emit_aof_rename, emit_aof_set,
    emit_aof_set_keepttl, emit_aof_set_pxat, AofHandle,
};
use crate::glob::Pattern;
use crate::info::{write_config_get, write_info, REDIS_VERSION};
use crate::protocol::{
    fmt_i64, fmt_u64, parse_canonical_i64, resolve_expiry, write_array_len, write_bulk,
    write_error, write_integer, write_map_len, write_nil, write_simple, Cmd, FlushMode,
    GetExOption, Protocol, SetCondition, SetExpiry, SetOptions, TimeOption, Value,
};
use crate::session::Session;
use crate::stats::Stats;
use crate::storage::{unix_ms, Dict, ExpireResult, GetExChange, GetExEffect, GetExResult};
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
#[inline(always)]
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
        let stats: Arc<Stats> = Arc::default();
        let mut dict = Dict::default();
        let (log, counters) = (aof.clone(), stats.clone());
        // A key removed because it expired is counted and logged as DEL,
        // unless the command replaces it anyway
        dict.set_on_expired(Box::new(move |key: &Bytes, replaced: bool| {
            counters.key_expired();
            if let (Some(aof), false) = (&log, replaced) {
                aof.write_owned(emit_aof_del(std::slice::from_ref(key)));
            }
        }));
        Self {
            id,
            dict,
            aof,
            stats,
        }
    }

    /// SET with options (also SETNX, GETSET, SETEX and PSETEX): sets the key
    /// if the condition allows, logs the SET as a replay needs it, and
    /// returns the old value (if asked for) and whether the key was set
    fn set_with(&self, key: Bytes, value: Bytes, options: SetOptions) -> (Option<Value>, bool) {
        let record = self.aof.as_ref().map(|_| match options.expiry {
            SetExpiry::Clear => emit_aof_set(&key, &value),
            SetExpiry::Keep => emit_aof_set_keepttl(&key, &value),
            SetExpiry::At(at) => emit_aof_set_pxat(&key, &value, at),
        });
        let (old, set) = self.dict.set_with(key, encode_value(value), options);
        if let (Some(a), Some(record), true) = (&self.aof, record, set) {
            a.write_owned(record);
        }
        (old, set)
    }

    /// Write the TTL family's reply for `key`: the time left, or the expiry
    /// itself if `absolute`, in milliseconds or rounded seconds
    fn ttl(&self, key: &[u8], millis: bool, absolute: bool, out: &mut BytesMut) {
        let reply = match self.dict.expiry(key) {
            None => -2,
            Some(0) => -1,
            Some(at) => {
                let at = i64::try_from(at).unwrap_or(i64::MAX);
                let ms = if absolute {
                    at
                } else {
                    let now = i64::try_from(unix_ms()).unwrap_or(i64::MAX);
                    (at - now).max(0)
                };
                if millis {
                    ms
                } else {
                    (ms + 500) / 1000
                }
            }
        };
        write_integer(reply, out);
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
        self.exec_frequent(cmd, Protocol::default(), out, |cmd, out| {
            self.exec_other(cmd, &mut Session::default(), out)
        });
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
        self.exec_frequent(cmd, session.protocol(), out, |cmd, out| {
            self.exec_other(cmd, session, out)
        });
    }

    /// Run `cmd` if it is one of the most frequent commands, which need no
    /// session, or hand it to `other`.
    ///
    /// They are kept out of the large match in `exec_other`: its big stack
    /// frame and the decoding of the command there would cost each of them
    /// dozens of instructions.
    #[inline(always)]
    fn exec_frequent(
        &self,
        cmd: Cmd,
        protocol: Protocol,
        out: &mut BytesMut,
        other: impl FnOnce(Cmd, &mut BytesMut),
    ) {
        match cmd {
            // PING [message] - connectivity test; echoes the message when given
            Cmd::Ping(None) => write_simple("PONG", out),
            Cmd::Ping(Some(message)) => write_bulk(&message, out),

            // GET key - retrieve value for key
            Cmd::Get(k) => write_get(&self.dict, &k, protocol, out),

            // SET key value - store key-value pair
            Cmd::Set(k, v) => self.set(k, v, out),

            // DEL / UNLINK key [key ...] - delete keys, reply with the number
            // removed (UNLINK is logged as DEL)
            Cmd::Del(keys) | Cmd::Unlink(keys) => self.del(keys, out),

            // EXISTS key [key ...] - count existing keys; repeated keys count
            // every time, like in Redis
            Cmd::Exists(keys) => {
                let mut existing = 0;
                self.dict
                    .read_many(&keys, |value| existing += value.is_some() as i64);
                write_integer(existing, out);
            }

            // INCR key, INCRBY / DECRBY / DECR - add to a numeric value
            Cmd::Incr(k) => self.incr_by(k, None, out),
            Cmd::IncrBy(k, delta) => self.incr_by(k, Some(delta), out),

            // MGET key1 key2 ... - get multiple keys
            Cmd::MGet(keys) => {
                write_array_len(keys.len(), out);
                write_mget(&self.dict, &keys, protocol, out);
            }

            // MSET key1 value1 key2 value2 ... - set multiple key-value pairs
            Cmd::MSet(pairs) => self.mset(pairs, out),

            cmd => other(cmd, out),
        }
    }

    /// SET key value
    #[inline(always)]
    fn set(&self, k: Bytes, v: Bytes, out: &mut BytesMut) {
        // Log to AOF if persistence is enabled, before moving k and v into
        // the dictionary
        if let Some(a) = &self.aof {
            a.write_owned(emit_aof_set(&k, &v));
        }
        self.dict.set(k, encode_value(v));
        write_simple("OK", out);
    }

    /// DEL key [key ...]
    fn del(&self, mut keys: Vec<Bytes>, out: &mut BytesMut) {
        // Keep only the keys that were removed; a repeated key is removed
        // (and counted) once, like in Redis.
        self.dict.del_many(&mut keys);
        if let Some(a) = &self.aof {
            if !keys.is_empty() {
                a.write_owned(emit_aof_del(&keys));
            }
        }
        write_integer(keys.len() as i64, out);
    }

    /// INCR key, or INCRBY key delta (also DECRBY and DECR)
    fn incr_by(&self, k: Bytes, delta: Option<i64>, out: &mut BytesMut) {
        // Keep the key for the AOF record only when persistence is on.
        let aof_key = self.aof.is_some().then(|| k.clone());
        match self.dict.incr_by(k, delta.unwrap_or(1)) {
            Ok(v) => {
                // Log only successful increments
                if let (Some(a), Some(key)) = (&self.aof, &aof_key) {
                    a.write_owned(match delta {
                        None => emit_aof_incr(key),
                        Some(delta) => emit_aof_incrby(key, delta),
                    });
                }
                write_integer(v, out);
            }
            Err(e) => write_error(e.as_str(), out),
        }
    }

    /// MSET key value [key value ...]
    fn mset(&self, pairs: Vec<(Bytes, Bytes)>, out: &mut BytesMut) {
        // Log all sets to AOF as a single operation
        if let Some(a) = &self.aof {
            a.write_owned(emit_aof_mset(&pairs));
        }
        // Set all key-value pairs at once
        self.dict.set_many(pairs, encode_value);
        write_simple("OK", out);
    }

    /// Execute every command but the most frequent ones, which
    /// `exec_frequent` runs
    #[inline(never)]
    fn exec_other(&self, cmd: Cmd, session: &mut Session, out: &mut BytesMut) {
        match cmd {
            // The most frequent commands, which `exec_frequent` runs before
            // any command gets here
            Cmd::Ping(_)
            | Cmd::Get(_)
            | Cmd::Set(..)
            | Cmd::Del(_)
            | Cmd::Unlink(_)
            | Cmd::Exists(_)
            | Cmd::Incr(_)
            | Cmd::IncrBy(..)
            | Cmd::MGet(_)
            | Cmd::MSet(_) => self.exec_frequent(cmd, session.protocol(), out, |_, _| ()),

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

            // EXPIRE / PEXPIRE / EXPIREAT / PEXPIREAT, logged as PEXPIREAT, or
            // as DEL when the time has passed and the key was deleted
            Cmd::Expire { key, at, options } => {
                let result = self.dict.expire(&key, at, options);
                if let Some(a) = &self.aof {
                    match result {
                        ExpireResult::Set => a.write_owned(emit_aof_pexpireat(&key, at)),
                        ExpireResult::Deleted => {
                            a.write_owned(emit_aof_del(std::slice::from_ref(&key)))
                        }
                        ExpireResult::Missing | ExpireResult::Unchanged => {}
                    }
                }
                let changed = matches!(result, ExpireResult::Set | ExpireResult::Deleted);
                write_integer(i64::from(changed), out);
            }

            // TTL / PTTL / EXPIRETIME / PEXPIRETIME key
            Cmd::Ttl(key) => self.ttl(&key, false, false, out),
            Cmd::PTtl(key) => self.ttl(&key, true, false, out),
            Cmd::ExpireTime(key) => self.ttl(&key, false, true, out),
            Cmd::PExpireTime(key) => self.ttl(&key, true, true, out),

            // SET key value [NX|XX] [GET] [EX|PX|EXAT|PXAT time|KEEPTTL],
            // SETEX and PSETEX
            Cmd::SetWith(key, value, options) => {
                let (old, set) = self.set_with(key, value, options);
                if options.get {
                    write_value(old.as_ref(), session.protocol(), out);
                } else if set {
                    write_simple("OK", out);
                } else {
                    write_nil(session.protocol(), out);
                }
            }

            // SETNX key value
            Cmd::SetNx(key, value) => {
                let options = SetOptions {
                    condition: Some(SetCondition::Nx),
                    ..SetOptions::default()
                };
                let (_, set) = self.set_with(key, value, options);
                write_integer(i64::from(set), out);
            }

            // GETSET key value
            Cmd::GetSet(key, value) => {
                let options = SetOptions {
                    get: true,
                    ..SetOptions::default()
                };
                let (old, _) = self.set_with(key, value, options);
                write_value(old.as_ref(), session.protocol(), out);
            }

            // GETDEL key, logged as DEL
            Cmd::GetDel(key) => {
                let value = self.dict.take(&key);
                if let (Some(a), Some(_)) = (&self.aof, &value) {
                    a.write_owned(emit_aof_del(std::slice::from_ref(&key)));
                }
                write_value(value.as_ref(), session.protocol(), out);
            }

            // GETEX key [EX|PX|EXAT|PXAT time|PERSIST], logged as
            // PEXPIREAT, PERSIST or DEL
            Cmd::GetEx(key, option) => {
                let change = || {
                    let (unit, time) = match &option {
                        None => return Ok(GetExChange::Keep),
                        Some(GetExOption::Persist) => return Ok(GetExChange::Persist),
                        Some(GetExOption::Ex(time)) => (TimeOption::Ex, time),
                        Some(GetExOption::Px(time)) => (TimeOption::Px, time),
                        Some(GetExOption::ExAt(time)) => (TimeOption::ExAt, time),
                        Some(GetExOption::PxAt(time)) => (TimeOption::PxAt, time),
                    };
                    let absolute = matches!(unit, TimeOption::ExAt | TimeOption::PxAt);
                    let at = resolve_expiry(unit, time, "getex")?;
                    Ok(GetExChange::ExpireAt { at, absolute })
                };
                match self.dict.get_ex(&key, change) {
                    GetExResult::Missing => write_nil(session.protocol(), out),
                    GetExResult::Invalid(error) => write_error(&error, out),
                    GetExResult::Done(value, effect) => {
                        if let Some(a) = &self.aof {
                            match effect {
                                GetExEffect::ExpiresAt(at) => {
                                    a.write_owned(emit_aof_pexpireat(&key, at))
                                }
                                GetExEffect::Persisted => a.write_owned(emit_aof_persist(&key)),
                                GetExEffect::Deleted => {
                                    a.write_owned(emit_aof_del(std::slice::from_ref(&key)))
                                }
                                GetExEffect::Unchanged => {}
                            }
                        }
                        write_value(Some(&value), session.protocol(), out);
                    }
                }
            }

            // MSETNX key value [key value ...], logged as MSET
            Cmd::MSetNx(pairs) => {
                let record = self.aof.as_ref().map(|_| emit_aof_mset(&pairs));
                let set = self.dict.set_many_if_absent(pairs, encode_value);
                if let (Some(a), Some(record), true) = (&self.aof, record, set) {
                    a.write_owned(record);
                }
                write_integer(i64::from(set), out);
            }

            // PERSIST key
            Cmd::Persist(key) => {
                let removed = self.dict.persist(&key);
                if let (Some(a), true) = (&self.aof, removed) {
                    a.write_owned(emit_aof_persist(&key));
                }
                write_integer(i64::from(removed), out);
            }

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
    fn expired_keys_are_counted_and_logged_as_del() {
        let dir = std::env::temp_dir().join(format!("ignix-shard-expiry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.aof");
        let aof = crate::aof::spawn_aof_writer(path.to_str().unwrap()).unwrap();
        let shard = Shard::new(0, Some(aof));
        let mut out = BytesMut::new();
        shard
            .dict
            .insert_expired(Bytes::from_static(b"old"), Value::Int(1));
        shard.exec(Cmd::Get(Bytes::from_static(b"old")), &mut out);
        assert_eq!(&out[..], b"$-1\r\n");
        assert_eq!(shard.stats.expired_keys(), 1);
        // A replaced key is counted but needs no record
        shard
            .dict
            .insert_expired(Bytes::from_static(b"reused"), Value::Int(1));
        shard.exec(
            Cmd::Set(Bytes::from_static(b"reused"), Bytes::from_static(b"2")),
            &mut out,
        );
        assert_eq!(shard.stats.expired_keys(), 2);
        shard.exec(
            Cmd::Set(Bytes::from_static(b"marker"), Bytes::from_static(b"end")),
            &mut out,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let data = loop {
            let data = std::fs::read(&path).unwrap_or_default();
            if data.ends_with(b"$6\r\nmarker\r\n$3\r\nend\r\n") {
                break data;
            }
            assert!(std::time::Instant::now() < deadline, "AOF: {data:?}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        drop(shard);
        let _ = std::fs::remove_dir_all(&dir);
        let text = String::from_utf8_lossy(&data);
        assert!(text.contains("*2\r\n$3\r\nDEL\r\n$3\r\nold\r\n"), "{text}");
        assert!(!text.contains("DEL\r\n$6\r\nreused"), "{text}");
    }

    #[test]
    fn test_shard_alignment() {
        assert_eq!(
            std::mem::align_of::<Shard>(),
            64,
            "Shard struct should be aligned to 64 bytes"
        );
    }
}
