/*!
 * Redis RESP Protocol Implementation
 *
 * This module implements the Redis Serialization Protocol (RESP) for parsing
 * and encoding commands and responses. It handles the complete protocol specification
 * including command parsing, validation, and response formatting.
 */

use crate::commands::{self, CommandSpec, Kind, SubcommandSpec};
use anyhow::{bail, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::ops::Range;

/// Largest element count accepted in one request (Redis rejects counts above `INT_MAX`).
const MAX_MULTIBULK_LEN: i64 = i32::MAX as i64;
/// Largest bulk string accepted in one request (Redis `proto-max-bulk-len` default: 512 MiB).
const MAX_BULK_LEN: i64 = 512 * 1024 * 1024;
/// Longest integer line Redis accepts: 20 bytes, e.g. `-9223372036854775808`.
const MAX_INT_LINE: usize = 20;
/// Smallest encoding of one element (`$0\r\n\r\n`), used to bound preallocation.
const MIN_ELEMENT_LEN: usize = 6;

const INVALID_MULTIBULK: &str = "ERR Protocol error: invalid multibulk length";
const INVALID_BULK: &str = "ERR Protocol error: invalid bulk length";

/// The RESP version a connection speaks, chosen with `HELLO`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Protocol {
    /// RESP2, which every connection starts with
    #[default]
    Resp2,
    /// RESP3
    Resp3,
}

/// How `FLUSHDB` and `FLUSHALL` free the memory of the removed keys
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushMode {
    /// Before replying (`SYNC`, the default)
    Sync,
    /// In a background thread (`ASYNC`)
    Async,
}

/// Whether SET only sets a key that is missing (NX) or one that exists (XX)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetCondition {
    /// `NX`: only if the key does not exist
    Nx,
    /// `XX`: only if the key exists
    Xx,
}

/// What SET does with the key's expiry
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SetExpiry {
    /// Remove it, the default
    #[default]
    Clear,
    /// Keep it (`KEEPTTL`)
    Keep,
    /// Set it to this unix time in milliseconds (`EX`, `PX`, `EXAT` or
    /// `PXAT`, converted when parsing); a time that has passed makes the new
    /// value expire at once
    At(i64),
}

/// Options of SET
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SetOptions {
    /// `NX` or `XX`
    pub condition: Option<SetCondition>,
    /// `GET`: reply with the old value instead of OK
    pub get: bool,
    /// What happens to the key's expiry
    pub expiry: SetExpiry,
}

/// The option of GETEX. The times are kept as given: like Redis, they are
/// only checked when the key exists.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GetExOption {
    /// `EX seconds`
    Ex(Bytes),
    /// `PX milliseconds`
    Px(Bytes),
    /// `EXAT unix-time-seconds`
    ExAt(Bytes),
    /// `PXAT unix-time-milliseconds`
    PxAt(Bytes),
    /// `PERSIST`: remove the expiry
    Persist,
}

/// Conditions of EXPIRE and its variants; with none set the expiry is
/// always set
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExpireOptions {
    /// `NX`: only if the key has no expiry
    pub nx: bool,
    /// `XX`: only if the key has an expiry
    pub xx: bool,
    /// `GT`: only if the new expiry is later (never for a key without one)
    pub gt: bool,
    /// `LT`: only if the new expiry is earlier (always for a key without one)
    pub lt: bool,
}

/// Client attributes set with `CLIENT SETINFO`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientInfo {
    /// `LIB-NAME`: the name of the client library
    LibName,
    /// `LIB-VER`: the version of the client library
    LibVer,
}

/// Which command names `COMMAND LIST FILTERBY` keeps
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandFilter {
    /// `MODULE name`: commands of a module (Ignix has none)
    Module(Bytes),
    /// `ACLCAT category`: commands in an ACL category
    AclCat(Bytes),
    /// `PATTERN pattern`: names matching a glob pattern, ignoring case
    Pattern(Bytes),
}

/// Redis-compatible commands supported by Ignix
///
/// Each variant represents a specific Redis command with its parameters.
/// Keys and values are `Bytes`, so both text and binary data are supported.
/// More commands will be added, so matches outside this crate need a
/// wildcard arm.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
// A tag byte of its own: without it the compiler hides the tag in spare
// values of a field (`SetExpiry`), and every match on a command decodes it
#[repr(u8)]
pub enum Cmd {
    /// PING \[message\] - test connectivity; echoes `message` when given
    Ping(Option<Bytes>),
    /// GET key - retrieve value for a key
    Get(Bytes),
    /// SET key value - set a key-value pair
    Set(Bytes, Bytes),
    /// DEL key [key ...] - delete keys, replying with the number removed
    Del(Vec<Bytes>),
    /// RENAME oldkey newkey - rename a key
    Rename(Bytes, Bytes),
    /// EXISTS key [key ...] - count how many of the keys exist
    Exists(Vec<Bytes>),
    /// INCR key - increment numeric value
    Incr(Bytes),
    /// INCRBY key increment - add `increment` to the numeric value; DECRBY
    /// and DECR are parsed as negative increments
    IncrBy(Bytes, i64),
    /// MGET key1 key2 ... - get multiple keys
    MGet(Vec<Bytes>),
    /// MSET key1 value1 key2 value2 ... - set multiple key-value pairs
    MSet(Vec<(Bytes, Bytes)>),
    /// ECHO message - reply with `message`
    Echo(Bytes),
    /// QUIT - reply OK and close the connection, dropping later requests
    Quit,
    /// SELECT index - switch to database `index`; only database 0 exists
    Select(i32),
    /// HELLO \[protover \[AUTH username password\] \[SETNAME clientname\]\] -
    /// switch the connection to `protocol` if given, and describe the server
    #[non_exhaustive]
    Hello {
        /// The protocol to switch to
        protocol: Option<Protocol>,
        /// The options after `protover`. Like Redis, they are applied in
        /// order when the command runs, so an option takes effect even if a
        /// later one fails.
        options: Vec<Bytes>,
    },
    /// CLIENT ID - the id of the connection
    ClientId,
    /// CLIENT GETNAME - the name of the connection, or null
    ClientGetName,
    /// CLIENT SETNAME name - name the connection; an empty name removes it
    ClientSetName(Bytes),
    /// CLIENT SETINFO LIB-NAME|LIB-VER value - record the client library's
    /// name or version
    ClientSetInfo(ClientInfo, Bytes),
    /// CLIENT HELP - describe the supported CLIENT subcommands
    ClientHelp,
    /// DBSIZE - the number of keys
    DbSize,
    /// TYPE key - the type of the value stored at `key`, or `none`
    Type(Bytes),
    /// UNLINK key [key ...] - delete keys like DEL
    Unlink(Vec<Bytes>),
    /// FLUSHDB \[ASYNC|SYNC\] - delete every key of the database
    FlushDb(FlushMode),
    /// FLUSHALL \[ASYNC|SYNC\] - delete every key of every database
    FlushAll(FlushMode),
    /// KEYS pattern - every key matching the glob `pattern`
    Keys(Bytes),
    /// SCAN cursor \[MATCH pattern\] \[COUNT count\] \[TYPE type\] - the next
    /// keys of an iteration over the keyspace
    #[non_exhaustive]
    Scan {
        /// Where the iteration goes on; 0 starts it
        cursor: u64,
        /// Only keys matching this glob pattern are returned
        pattern: Option<Bytes>,
        /// About how many keys to look at (10 unless given)
        count: usize,
        /// Only keys holding values of this type are returned
        type_name: Option<Bytes>,
    },
    /// INFO \[section ...\] - describe the server
    Info(Vec<Bytes>),
    /// CONFIG GET parameter \[parameter ...\] - configuration parameters
    /// matching the names or glob patterns, with their values
    ConfigGet(Vec<Bytes>),
    /// CONFIG HELP - describe the supported CONFIG subcommands
    ConfigHelp,
    /// EXPIRE / PEXPIRE / EXPIREAT / PEXPIREAT key time \[NX|XX|GT|LT\] -
    /// set the key's expiry
    #[non_exhaustive]
    Expire {
        /// The key
        key: Bytes,
        /// The new expiry as a unix time in milliseconds (relative times are
        /// converted when parsing); a time not after now deletes the key
        at: i64,
        /// When to set it
        options: ExpireOptions,
    },
    /// TTL key - seconds until the key expires (-1 without an expiry, -2 if
    /// the key is missing)
    Ttl(Bytes),
    /// PTTL key - milliseconds until the key expires
    PTtl(Bytes),
    /// EXPIRETIME key - the key's expiry as a unix time in seconds
    ExpireTime(Bytes),
    /// PEXPIRETIME key - the key's expiry as a unix time in milliseconds
    PExpireTime(Bytes),
    /// PERSIST key - remove the key's expiry
    Persist(Bytes),
    /// SET key value with options; also SETEX and PSETEX, which set an
    /// expiry
    SetWith(Bytes, Bytes, SetOptions),
    /// SETNX key value - set the key if it does not exist
    SetNx(Bytes, Bytes),
    /// GETSET key value - set the key and reply with its old value
    GetSet(Bytes, Bytes),
    /// GETDEL key - reply with the value and delete the key
    GetDel(Bytes),
    /// GETEX key \[EX|PX|EXAT|PXAT time | PERSIST\] - reply with the value
    /// and change the key's expiry
    GetEx(Bytes, Option<GetExOption>),
    /// MSETNX key value \[key value ...\] - set every pair if none of the
    /// keys exists
    MSetNx(Vec<(Bytes, Bytes)>),
    /// AUTH \[username\] password - authenticate the connection; the only
    /// user is `default`
    #[non_exhaustive]
    Auth {
        /// The user, when given
        username: Option<Bytes>,
        /// The password
        password: Bytes,
    },
    /// COMMAND and COMMAND INFO \[name ...\] - describe the named commands,
    /// or every command without names
    CommandInfo(Vec<Bytes>),
    /// COMMAND COUNT - the number of commands
    CommandCount,
    /// COMMAND LIST \[FILTERBY MODULE|ACLCAT|PATTERN value\] - command names
    CommandList(Option<CommandFilter>),
    /// COMMAND GETKEYS command \[arg ...\] - the key arguments of a command
    CommandGetKeys(Vec<Bytes>),
    /// COMMAND HELP
    CommandHelp,
}

impl Cmd {
    /// Whether a client that has not authenticated yet may run the command
    /// (Redis `no-auth` commands: AUTH, HELLO and QUIT)
    pub(crate) fn allowed_before_auth(&self) -> bool {
        matches!(self, Cmd::Auth { .. } | Cmd::Hello { .. } | Cmd::Quit)
    }
}

/// Value types that can be stored in Ignix
///
/// Supports different data types while maintaining Redis compatibility.
/// More types will be added, so matches outside this crate need a wildcard
/// arm.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Value {
    /// String/binary data
    Str(Bytes),
    /// 64-bit signed integer
    Int(i64),
    /// Binary blob (same as Str but semantically different)
    Blob(Bytes),
}

/// Parse a signed 64-bit integer exactly like Redis `string2ll`.
///
/// Accepts an optional `-` followed by digits without leading zeros (`0`
/// itself is allowed), at most 20 bytes in total, and the value must fit in an
/// `i64`. `+`, spaces, `-0` and other non-canonical forms are rejected, so every
/// accepted input formats back to the same bytes.
pub(crate) fn parse_canonical_i64(s: &[u8]) -> Option<i64> {
    if s.is_empty() || s.len() > MAX_INT_LINE {
        return None;
    }
    if s == b"0" {
        return Some(0);
    }
    let (negative, digits) = match s {
        [b'-', rest @ ..] => (true, rest),
        _ => (false, s),
    };
    if !matches!(digits.first(), Some(b'1'..=b'9')) {
        return None;
    }
    let mut magnitude: u64 = 0;
    for &c in digits {
        if !c.is_ascii_digit() {
            return None;
        }
        magnitude = magnitude
            .checked_mul(10)?
            .checked_add(u64::from(c - b'0'))?;
    }
    if negative {
        // i64::MIN has no positive counterpart, so negate in the unsigned domain.
        if magnitude > i64::MIN.unsigned_abs() {
            return None;
        }
        Some(0i64.wrapping_sub_unsigned(magnitude))
    } else {
        i64::try_from(magnitude).ok()
    }
}

/// Read the integer of a `*<n>\r\n` or `$<n>\r\n` header whose digits start at `pos`.
///
/// Returns `Ok(None)` while the line is incomplete, the position after the
/// line and the value once it is complete, and `invalid` as the error when the
/// line is malformed.
///
/// Always inlined: it runs for every header, and left to itself the compiler
/// stopped inlining it after unrelated changes to the parser.
#[inline(always)]
fn read_int_line(data: &[u8], pos: usize, invalid: &'static str) -> Result<Option<(usize, i64)>> {
    let rest = &data[pos..];
    let window = &rest[..rest.len().min(MAX_INT_LINE + 1)];
    let Some(cr) = window.iter().position(|&b| b == b'\r') else {
        if rest.len() > MAX_INT_LINE {
            bail!(invalid);
        }
        return Ok(None);
    };
    match rest.get(cr + 1) {
        None => Ok(None),
        Some(b'\n') => match parse_canonical_i64(&rest[..cr]) {
            Some(value) => Ok(Some((pos + cr + 2, value))),
            None => bail!(invalid),
        },
        Some(_) => bail!(invalid),
    }
}

/// A complete request frame at the start of a buffer
struct Frame {
    /// Its length in bytes
    len: usize,
    /// Where its command name is in the buffer, and the arguments after it;
    /// `None` for a frame of no elements, which Redis ignores
    request: Option<(Range<usize>, Vec<Bytes>)>,
}

/// Read one complete request frame: `*<n>\r\n` followed by `n` bulk strings.
///
/// Returns `Ok(None)` when more data is needed. Malformed input is an error
/// whose message is a complete RESP error line (`ERR Protocol error: ...`).
/// The command name is left in `data`, and only the arguments are copied:
/// looking the command up needs no copy of its own.
fn read_frame(data: &[u8]) -> Result<Option<Frame>> {
    RequestParser::default().next_frame(data)
}

/// Incremental request parser for one connection
///
/// Works like [`parse_requests`], but remembers how far a request that has
/// not fully arrived was already parsed, so each call only looks at the new
/// bytes: a large request that arrives in many small reads costs time linear
/// in its size. `parse_requests` starts a partial request over on every call
/// and copies its arguments again each time.
#[derive(Debug, Default)]
pub struct RequestParser {
    /// Offset in the buffer of the next unparsed element; 0 when no request
    /// is in progress
    cursor: usize,
    /// Elements of the current request still to be read
    remaining: usize,
    /// Where the command name of the current request is in the buffer, once
    /// it has been read
    name: Option<Range<usize>>,
    /// Arguments of the current request read so far, after the name
    args: Vec<Bytes>,
}

impl RequestParser {
    /// A parser with no request in progress
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse every complete request in `buf` into `out`, consuming its bytes
    ///
    /// Same results and errors as [`parse_requests`]. Between calls `buf` may
    /// only grow at the end: the bytes of a partial request must stay where
    /// they are. After an error, or when the buffer is cleared, call
    /// [`RequestParser::reset`] before parsing again.
    pub fn parse(&mut self, buf: &mut BytesMut, out: &mut Vec<Request>) -> Result<()> {
        self.parse_with(buf, out, into_request)
    }

    /// Like [`RequestParser::parse`], but keeps apart the invalid requests
    /// that Redis rejects only once the client has authenticated
    pub(crate) fn parse_split(&mut self, buf: &mut BytesMut, out: &mut Vec<Parsed>) -> Result<()> {
        self.parse_with(buf, out, |parsed| parsed)
    }

    fn parse_with<T>(
        &mut self,
        buf: &mut BytesMut,
        out: &mut Vec<T>,
        convert: impl Fn(Parsed) -> T,
    ) -> Result<()> {
        loop {
            let Some(frame) = self.next_frame(&buf[..])? else {
                return Ok(());
            };
            if let Some((name, args)) = frame.request {
                out.push(convert(command_from_frame(&buf[name], args)));
            }
            buf.advance(frame.len);
        }
    }

    /// Forget the request in progress
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Continue reading the frame at the start of `data`; see [`read_frame`].
    fn next_frame(&mut self, data: &[u8]) -> Result<Option<Frame>> {
        // The caller removed bytes it should have kept: start over.
        if self.cursor > data.len() {
            self.reset();
        }
        if self.cursor == 0 {
            let Some(&first) = data.first() else {
                return Ok(None);
            };
            if first != b'*' {
                bail!(
                    "ERR Protocol error: expected '*', got '{}'",
                    first.escape_ascii()
                );
            }
            let Some((cursor, count)) = read_int_line(data, 1, INVALID_MULTIBULK)? else {
                return Ok(None);
            };
            if count > MAX_MULTIBULK_LEN {
                bail!(INVALID_MULTIBULK);
            }
            if count <= 0 {
                return Ok(Some(Frame {
                    len: cursor,
                    request: None,
                }));
            }
            let count = count as usize;
            // Room for the arguments after the name. Do not trust the
            // announced count for preallocation: every element needs at least
            // MIN_ELEMENT_LEN bytes of input that must actually arrive.
            self.args =
                Vec::with_capacity((count - 1).min((data.len() - cursor) / MIN_ELEMENT_LEN));
            self.remaining = count;
            self.cursor = cursor;
        }
        while self.remaining > 0 {
            let cursor = self.cursor;
            let Some(&prefix) = data.get(cursor) else {
                return Ok(None);
            };
            if prefix != b'$' {
                bail!(
                    "ERR Protocol error: expected '$', got '{}'",
                    prefix.escape_ascii()
                );
            }
            let Some((start, len)) = read_int_line(data, cursor + 1, INVALID_BULK)? else {
                return Ok(None);
            };
            if !(0..=MAX_BULK_LEN).contains(&len) {
                bail!(INVALID_BULK);
            }
            let end = start + len as usize;
            let Some(terminator) = data.get(end..end + 2) else {
                return Ok(None);
            };
            if terminator != b"\r\n" {
                bail!(INVALID_BULK);
            }
            if self.name.is_none() {
                self.name = Some(start..end);
            } else {
                self.args.push(Bytes::copy_from_slice(&data[start..end]));
            }
            self.cursor = end + 2;
            self.remaining -= 1;
        }
        let len = std::mem::take(&mut self.cursor);
        let request = self
            .name
            .take()
            .map(|name| (name, std::mem::take(&mut self.args)));
        Ok(Some(Frame { len, request }))
    }
}

/// Why a well-formed request is not a valid command, split by when Redis
/// rejects it. Holds the complete error line to reply with.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Refusal {
    /// Rejected whether or not the client has authenticated: an unknown
    /// command or subcommand, a wrong number of arguments, or an invalid
    /// argument of a command allowed before authenticating
    Invalid(String),
    /// Rejected by the command's own checks, which Redis only reaches for an
    /// authenticated client; another client gets `NOAUTH` instead
    Rejected(String),
}

impl Refusal {
    /// The error line
    fn into_message(self) -> String {
        match self {
            Refusal::Invalid(message) | Refusal::Rejected(message) => message,
        }
    }

    /// The error as `parse_one` and `parse_many` return it
    fn into_error(self) -> anyhow::Error {
        anyhow::Error::msg(self.into_message())
    }
}

/// The errors of a command's own checks are [`Refusal::Rejected`]
impl From<String> for Refusal {
    fn from(message: String) -> Self {
        Refusal::Rejected(message)
    }
}

/// A request as the server sees it: like [`Request`], with the invalid
/// requests split by when Redis rejects them
///
/// A plain `Result`, so that the command the parser builds is written in
/// place: moving it into another enum copies all of it.
pub(crate) type Parsed = std::result::Result<Cmd, Refusal>;

/// The public form of a parsed request
fn into_request(parsed: Parsed) -> Request {
    match parsed {
        Ok(cmd) => Request::Cmd(cmd),
        Err(refusal) => Request::Invalid(refusal.into_message()),
    }
}

/// Build a command from the name and the other arguments (`items`) of one
/// request frame, or say why it is refused with the error line Redis 7
/// replies with.
///
/// The command name, the number of arguments and the subcommand are checked
/// first, as Redis does before it checks authentication.
fn command_from_frame(name: &[u8], items: Vec<Bytes>) -> Parsed {
    let Some(spec) = commands::lookup(name) else {
        return Err(Refusal::Invalid(unknown_command_error(name, &items)));
    };
    if !spec.arity_matches(items.len() + 1) {
        return Err(Refusal::Invalid(arity_error(spec.name)));
    }
    if matches!(spec.kind, Kind::Client | Kind::Config | Kind::Command) && !items.is_empty() {
        if let Err(error) = subcommand(spec, &items) {
            return Err(Refusal::Invalid(error));
        }
    }
    build_command(spec, items)
}

/// `ERR wrong number of arguments for '<name>' command`
fn arity_error(name: &str) -> String {
    format!("ERR wrong number of arguments for '{name}' command")
}

/// Build the command `spec` from the arguments after its name, whose number
/// `command_from_frame` has checked, with the checks Redis makes in the
/// command itself.
///
/// Their errors are [`Refusal::Rejected`], except for the commands allowed
/// before authenticating (`AUTH`, `HELLO`, `QUIT`): Redis reports those to any
/// client.
#[inline(always)]
fn build_command(spec: &CommandSpec, mut items: Vec<Bytes>) -> Parsed {
    let kind = spec.kind;
    // Counted like Redis, with the command name
    let argc = items.len() + 1;
    let arity_error = || arity_error(spec.name);
    // Limits Redis checks in the commands themselves, with the same error
    let arity_ok = match kind {
        Kind::Ping => argc <= 2,
        Kind::MSet | Kind::MSetNx => argc % 2 == 1,
        _ => true,
    };
    if !arity_ok {
        return Err(arity_error().into());
    }

    let cmd = match kind {
        Kind::Ping => Cmd::Ping(items.pop()),
        Kind::Del => Cmd::Del(items),
        Kind::Unlink => Cmd::Unlink(items),
        Kind::DbSize => Cmd::DbSize,
        Kind::Type => {
            let [key] = <[Bytes; 1]>::try_from(items).map_err(|_| arity_error())?;
            Cmd::Type(key)
        }
        Kind::FlushDb | Kind::FlushAll => {
            let mode = match &items[..] {
                [] => FlushMode::Sync,
                [option] if option.eq_ignore_ascii_case(b"sync") => FlushMode::Sync,
                [option] if option.eq_ignore_ascii_case(b"async") => FlushMode::Async,
                _ => return Err("ERR syntax error".to_string().into()),
            };
            if kind == Kind::FlushDb {
                Cmd::FlushDb(mode)
            } else {
                Cmd::FlushAll(mode)
            }
        }
        Kind::Exists => Cmd::Exists(items),
        Kind::MGet => Cmd::MGet(items),
        Kind::MSet | Kind::MSetNx => {
            let mut pairs = Vec::with_capacity(items.len() / 2);
            let mut args = items.into_iter();
            while let (Some(key), Some(value)) = (args.next(), args.next()) {
                pairs.push((key, value));
            }
            if kind == Kind::MSet {
                Cmd::MSet(pairs)
            } else {
                Cmd::MSetNx(pairs)
            }
        }
        Kind::Get | Kind::Incr | Kind::Decr => {
            let [key] = <[Bytes; 1]>::try_from(items).map_err(|_| arity_error())?;
            match kind {
                Kind::Get => Cmd::Get(key),
                Kind::Incr => Cmd::Incr(key),
                _ => Cmd::IncrBy(key, -1),
            }
        }
        Kind::IncrBy | Kind::DecrBy => {
            let [key, amount] = <[Bytes; 2]>::try_from(items).map_err(|_| arity_error())?;
            let amount = parse_canonical_i64(&amount)
                .ok_or_else(|| "ERR value is not an integer or out of range".to_string())?;
            let delta = if matches!(kind, Kind::IncrBy) {
                amount
            } else {
                amount
                    .checked_neg()
                    .ok_or_else(|| "ERR decrement would overflow".to_string())?
            };
            Cmd::IncrBy(key, delta)
        }
        Kind::Set if items.len() == 2 => {
            let [key, value] = <[Bytes; 2]>::try_from(items).map_err(|_| arity_error())?;
            Cmd::Set(key, value)
        }
        Kind::Set => {
            let parsed = string_options(&items[2..], true)?;
            let expiry = match parsed.time {
                Some((time, value)) => SetExpiry::At(resolve_expiry(time, &value, "set")?),
                None if parsed.keepttl => SetExpiry::Keep,
                None => SetExpiry::Clear,
            };
            let condition = match (parsed.nx, parsed.xx) {
                (true, _) => Some(SetCondition::Nx),
                (_, true) => Some(SetCondition::Xx),
                _ => None,
            };
            let options = SetOptions {
                condition,
                get: parsed.get,
                expiry,
            };
            let mut items = items;
            items.truncate(2);
            let [key, value] = <[Bytes; 2]>::try_from(items).map_err(|_| arity_error())?;
            Cmd::SetWith(key, value, options)
        }
        Kind::SetEx | Kind::PSetEx => {
            let [key, time, value] = <[Bytes; 3]>::try_from(items).map_err(|_| arity_error())?;
            let unit = if kind == Kind::SetEx {
                TimeOption::Ex
            } else {
                TimeOption::Px
            };
            let options = SetOptions {
                expiry: SetExpiry::At(resolve_expiry(unit, &time, spec.name)?),
                ..SetOptions::default()
            };
            Cmd::SetWith(key, value, options)
        }
        Kind::SetNx | Kind::GetSet => {
            let [key, value] = <[Bytes; 2]>::try_from(items).map_err(|_| arity_error())?;
            if kind == Kind::SetNx {
                Cmd::SetNx(key, value)
            } else {
                Cmd::GetSet(key, value)
            }
        }
        Kind::GetDel => {
            let [key] = <[Bytes; 1]>::try_from(items).map_err(|_| arity_error())?;
            Cmd::GetDel(key)
        }
        Kind::GetEx => {
            let parsed = string_options(&items[1..], false)?;
            let option = match parsed.time {
                Some((TimeOption::Ex, time)) => Some(GetExOption::Ex(time)),
                Some((TimeOption::Px, time)) => Some(GetExOption::Px(time)),
                Some((TimeOption::ExAt, time)) => Some(GetExOption::ExAt(time)),
                Some((TimeOption::PxAt, time)) => Some(GetExOption::PxAt(time)),
                None => parsed.persist.then_some(GetExOption::Persist),
            };
            let mut items = items;
            items.truncate(1);
            Cmd::GetEx(items.pop().unwrap_or_default(), option)
        }
        Kind::Rename => {
            let [from, to] = <[Bytes; 2]>::try_from(items).map_err(|_| arity_error())?;
            Cmd::Rename(from, to)
        }
        Kind::Echo => {
            let [message] = <[Bytes; 1]>::try_from(items).map_err(|_| arity_error())?;
            Cmd::Echo(message)
        }
        Kind::Quit => Cmd::Quit,
        Kind::Keys => {
            let [pattern] = <[Bytes; 1]>::try_from(items).map_err(|_| arity_error())?;
            Cmd::Keys(pattern)
        }
        Kind::Scan => scan_command(items)?,
        Kind::Info => Cmd::Info(items),
        Kind::Expire | Kind::PExpire | Kind::ExpireAt | Kind::PExpireAt => {
            expire_command(kind, spec.name, items)?
        }
        Kind::Ttl | Kind::PTtl | Kind::ExpireTime | Kind::PExpireTime | Kind::Persist => {
            let [key] = <[Bytes; 1]>::try_from(items).map_err(|_| arity_error())?;
            match kind {
                Kind::Ttl => Cmd::Ttl(key),
                Kind::PTtl => Cmd::PTtl(key),
                Kind::ExpireTime => Cmd::ExpireTime(key),
                Kind::PExpireTime => Cmd::PExpireTime(key),
                _ => Cmd::Persist(key),
            }
        }
        Kind::Config => config_command(spec, items)?,
        Kind::Client => client_command(spec, items)?,
        Kind::Command => command_command(spec, items)?,
        Kind::Hello => {
            let mut args = items.into_iter();
            let protocol = match args.next() {
                None => None,
                Some(version) => match parse_canonical_i64(&version) {
                    Some(2) => Some(Protocol::Resp2),
                    Some(3) => Some(Protocol::Resp3),
                    Some(_) => {
                        return Err(Refusal::Invalid(
                            "NOPROTO unsupported protocol version".to_string(),
                        ))
                    }
                    None => {
                        return Err(Refusal::Invalid(
                            "ERR Protocol version is not an integer or out of range".to_string(),
                        ))
                    }
                },
            };
            Cmd::Hello {
                protocol,
                options: args.collect(),
            }
        }
        Kind::Auth => {
            let mut args = items.into_iter();
            match (args.next(), args.next(), args.next()) {
                (Some(password), None, _) => Cmd::Auth {
                    username: None,
                    password,
                },
                (Some(username), Some(password), None) => Cmd::Auth {
                    username: Some(username),
                    password,
                },
                _ => return Err(Refusal::Invalid("ERR syntax error".to_string())),
            }
        }
        Kind::Select => {
            let [index] = <[Bytes; 1]>::try_from(items).map_err(|_| arity_error())?;
            let index = parse_canonical_i64(&index)
                .ok_or_else(|| "ERR value is not an integer or out of range".to_string())?;
            let index = i32::try_from(index).map_err(|_| {
                "ERR value is out of range, value must between -2147483648 and 2147483647"
                    .to_string()
            })?;
            Cmd::Select(index)
        }
    };
    Ok(cmd)
}

/// Parse a SCAN cursor like Redis: `strtoul` in base 10 with nothing left
/// over, so an empty cursor is 0 and `-1` wraps around, while a leading
/// space or an overflow makes it invalid.
fn parse_scan_cursor(s: &[u8]) -> Option<u64> {
    let (negative, digits) = match s {
        [] => return Some(0),
        [b'+', rest @ ..] => (false, rest),
        [b'-', rest @ ..] => (true, rest),
        _ => (false, s),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut value: u64 = 0;
    for &digit in digits {
        value = value
            .checked_mul(10)?
            .checked_add(u64::from(digit - b'0'))?;
    }
    Some(if negative {
        value.wrapping_neg()
    } else {
        value
    })
}

/// Build a SCAN command from the arguments after `SCAN`.
fn scan_command(items: Vec<Bytes>) -> std::result::Result<Cmd, String> {
    let cursor = parse_scan_cursor(&items[0]).ok_or_else(|| "ERR invalid cursor".to_string())?;
    let (mut pattern, mut count, mut type_name) = (None, 10, None);
    let options = &items[1..];
    let mut i = 0;
    while i < options.len() {
        let option = &options[i];
        let Some(value) = options.get(i + 1) else {
            return Err("ERR syntax error".to_string());
        };
        if option.eq_ignore_ascii_case(b"count") {
            let n = parse_canonical_i64(value)
                .ok_or_else(|| "ERR value is not an integer or out of range".to_string())?;
            if n < 1 {
                return Err("ERR syntax error".to_string());
            }
            count = usize::try_from(n).unwrap_or(usize::MAX);
        } else if option.eq_ignore_ascii_case(b"match") {
            pattern = Some(value.clone());
        } else if option.eq_ignore_ascii_case(b"type") {
            type_name = Some(value.clone());
        } else {
            return Err("ERR syntax error".to_string());
        }
        i += 2;
    }
    Ok(Cmd::Scan {
        cursor,
        pattern,
        count,
        type_name,
    })
}

/// Parse the NX, XX, GT and LT options of EXPIRE, with Redis's errors
fn expire_options(options: &[Bytes]) -> std::result::Result<ExpireOptions, String> {
    let mut parsed = ExpireOptions::default();
    for option in options {
        let flag = match option.to_ascii_lowercase().as_slice() {
            b"nx" => &mut parsed.nx,
            b"xx" => &mut parsed.xx,
            b"gt" => &mut parsed.gt,
            b"lt" => &mut parsed.lt,
            _ => {
                let option = String::from_utf8_lossy(option);
                return Err(format!("ERR Unsupported option {option}"));
            }
        };
        *flag = true;
    }
    if parsed.nx && (parsed.xx || parsed.gt || parsed.lt) {
        return Err(
            "ERR NX and XX, GT or LT options at the same time are not compatible".to_string(),
        );
    }
    if parsed.gt && parsed.lt {
        return Err("ERR GT and LT options at the same time are not compatible".to_string());
    }
    Ok(parsed)
}

/// Build EXPIRE, PEXPIRE, EXPIREAT or PEXPIREAT from the arguments after the
/// command name, converting the time to an absolute unix time in
/// milliseconds with the overflow checks Redis makes.
fn expire_command(kind: Kind, name: &str, items: Vec<Bytes>) -> std::result::Result<Cmd, String> {
    let options = expire_options(&items[2..])?;
    let time = parse_canonical_i64(&items[1])
        .ok_or_else(|| "ERR value is not an integer or out of range".to_string())?;
    let invalid = || format!("ERR invalid expire time in '{name}' command");
    let millis = match kind {
        Kind::Expire | Kind::ExpireAt => time.checked_mul(1000).ok_or_else(invalid)?,
        _ => time,
    };
    let at = match kind {
        Kind::Expire | Kind::PExpire => {
            let now = i64::try_from(crate::storage::unix_ms()).unwrap_or(i64::MAX);
            millis.checked_add(now).ok_or_else(invalid)?
        }
        _ => millis,
    };
    let mut items = items;
    items.truncate(1);
    Ok(Cmd::Expire {
        key: items.pop().unwrap_or_default(),
        at,
        options,
    })
}

/// The subcommand of `command` (CLIENT, CONFIG or COMMAND) named first in
/// `items`, the arguments after the command name, once it is known to exist
/// and to have the right number of arguments
fn subcommand(
    command: &CommandSpec,
    items: &[Bytes],
) -> std::result::Result<&'static SubcommandSpec, String> {
    let name = &items[0];
    let Some(spec) = commands::find_subcommand(command.kind, name) else {
        let shown = String::from_utf8_lossy(&name[..name.len().min(128)]);
        let command = command.name.to_ascii_uppercase();
        return Err(format!(
            "ERR unknown subcommand '{shown}'. Try {command} HELP."
        ));
    };
    if !spec.arity_matches(items.len() + 1) {
        return Err(arity_error(spec.name));
    }
    Ok(spec)
}

/// Build a `CONFIG` subcommand from the arguments after `CONFIG`.
fn config_command(spec: &CommandSpec, mut items: Vec<Bytes>) -> std::result::Result<Cmd, String> {
    if subcommand(spec, &items)?.short_name() == "help" {
        return Ok(Cmd::ConfigHelp);
    }
    items.remove(0);
    Ok(Cmd::ConfigGet(items))
}

/// Build a `COMMAND` subcommand from the arguments after `COMMAND`.
fn command_command(spec: &CommandSpec, mut items: Vec<Bytes>) -> std::result::Result<Cmd, String> {
    if items.is_empty() {
        return Ok(Cmd::CommandInfo(items));
    }
    let name = subcommand(spec, &items)?.short_name();
    items.remove(0);
    Ok(match name {
        "count" => Cmd::CommandCount,
        "info" => Cmd::CommandInfo(items),
        "getkeys" => Cmd::CommandGetKeys(items),
        "help" => Cmd::CommandHelp,
        _ => {
            // LIST [FILTERBY MODULE|ACLCAT|PATTERN value]
            let filter = match <[Bytes; 3]>::try_from(items) {
                Err(items) if items.is_empty() => None,
                Ok([filterby, kind, value]) if filterby.eq_ignore_ascii_case(b"filterby") => {
                    if kind.eq_ignore_ascii_case(b"module") {
                        Some(CommandFilter::Module(value))
                    } else if kind.eq_ignore_ascii_case(b"aclcat") {
                        Some(CommandFilter::AclCat(value))
                    } else if kind.eq_ignore_ascii_case(b"pattern") {
                        Some(CommandFilter::Pattern(value))
                    } else {
                        return Err("ERR syntax error".to_string());
                    }
                }
                _ => return Err("ERR syntax error".to_string()),
            };
            Cmd::CommandList(filter)
        }
    })
}

/// Build a `CLIENT` subcommand from the arguments after `CLIENT`.
fn client_command(spec: &CommandSpec, mut items: Vec<Bytes>) -> std::result::Result<Cmd, String> {
    let name = subcommand(spec, &items)?.short_name();
    Ok(match name {
        "id" => Cmd::ClientId,
        "getname" => Cmd::ClientGetName,
        "help" => Cmd::ClientHelp,
        "setname" => Cmd::ClientSetName(items.pop().unwrap_or_default()),
        _ => {
            let value = items.pop().unwrap_or_default();
            let attribute = &items[1];
            let info = if attribute.eq_ignore_ascii_case(b"lib-name") {
                ClientInfo::LibName
            } else if attribute.eq_ignore_ascii_case(b"lib-ver") {
                ClientInfo::LibVer
            } else {
                let attribute = String::from_utf8_lossy(attribute);
                return Err(format!("ERR Unrecognized option '{attribute}'"));
            };
            if !is_printable_ascii(&value) {
                let attribute = String::from_utf8_lossy(attribute);
                return Err(format!(
                    "ERR {attribute} cannot contain spaces, newlines or special characters."
                ));
            }
            Cmd::ClientSetInfo(info, value)
        }
    })
}

/// Whether `value` only has printable ASCII characters other than space, as
/// client names and attributes must (so `CLIENT LIST` can split on spaces)
pub(crate) fn is_printable_ascii(value: &[u8]) -> bool {
    value.iter().all(|b| (b'!'..=b'~').contains(b))
}

/// `ERR unknown command ...`, truncated like Redis: the name is cut to 128
/// bytes, and quoted arguments are appended while that part is shorter than
/// 128 bytes (each one cut to the remaining room).
fn unknown_command_error(name: &[u8], args: &[Bytes]) -> String {
    const LIMIT: usize = 128;
    let mut msg = Vec::with_capacity(96);
    msg.extend_from_slice(b"ERR unknown command '");
    msg.extend_from_slice(&name[..name.len().min(LIMIT)]);
    msg.extend_from_slice(b"', with args beginning with: ");
    let args_start = msg.len();
    for arg in args {
        let used = msg.len() - args_start;
        if used >= LIMIT {
            break;
        }
        msg.push(b'\'');
        msg.extend_from_slice(&arg[..arg.len().min(LIMIT - used)]);
        msg.extend_from_slice(b"' ");
    }
    String::from_utf8_lossy(&msg).into_owned()
}

/// The time options of SET and GETEX
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimeOption {
    /// `EX seconds`
    Ex,
    /// `PX milliseconds`
    Px,
    /// `EXAT unix-time-seconds`
    ExAt,
    /// `PXAT unix-time-milliseconds`
    PxAt,
}

/// The options of SET or GETEX, before the time is checked
#[derive(Default)]
struct StringOptions {
    nx: bool,
    xx: bool,
    get: bool,
    keepttl: bool,
    persist: bool,
    time: Option<(TimeOption, Bytes)>,
}

/// Parse the options of SET (`set`) or GETEX with Redis's rules: NX, XX,
/// GET and KEEPTTL are for SET and PERSIST for GETEX; NX and XX exclude each
/// other; one of EX, PX, EXAT and PXAT may be given (repeatedly), but not
/// with KEEPTTL or PERSIST. Anything else is a syntax error.
fn string_options(options: &[Bytes], set: bool) -> std::result::Result<StringOptions, String> {
    let mut parsed = StringOptions::default();
    let mut i = 0;
    while i < options.len() {
        let option = options[i].to_ascii_lowercase();
        let time = match option.as_slice() {
            b"ex" => Some(TimeOption::Ex),
            b"px" => Some(TimeOption::Px),
            b"exat" => Some(TimeOption::ExAt),
            b"pxat" => Some(TimeOption::PxAt),
            _ => None,
        };
        let other_time = |kind| {
            parsed
                .time
                .as_ref()
                .is_some_and(|(given, _)| *given != kind)
        };
        match (option.as_slice(), time) {
            (b"nx", _) if set && !parsed.xx => parsed.nx = true,
            (b"xx", _) if set && !parsed.nx => parsed.xx = true,
            (b"get", _) if set => parsed.get = true,
            (b"keepttl", _) if set && !parsed.persist && parsed.time.is_none() => {
                parsed.keepttl = true
            }
            (b"persist", _) if !set && !parsed.keepttl && parsed.time.is_none() => {
                parsed.persist = true
            }
            (_, Some(kind)) if !parsed.keepttl && !parsed.persist && !other_time(kind) => {
                let Some(value) = options.get(i + 1) else {
                    return Err("ERR syntax error".to_string());
                };
                parsed.time = Some((kind, value.clone()));
                i += 1;
            }
            _ => return Err("ERR syntax error".to_string()),
        }
        i += 1;
    }
    Ok(parsed)
}

/// The unix time in milliseconds a SET, SETEX, PSETEX or GETEX time stands
/// for, checked like Redis: the number must be positive and the result must
/// not overflow. `command` names the command in the error.
pub(crate) fn resolve_expiry(
    kind: TimeOption,
    time: &[u8],
    command: &str,
) -> std::result::Result<i64, String> {
    let value = parse_canonical_i64(time)
        .ok_or_else(|| "ERR value is not an integer or out of range".to_string())?;
    let invalid = || format!("ERR invalid expire time in '{command}' command");
    if value <= 0 {
        return Err(invalid());
    }
    let millis = match kind {
        TimeOption::Ex | TimeOption::ExAt => value.checked_mul(1000).ok_or_else(invalid)?,
        TimeOption::Px | TimeOption::PxAt => value,
    };
    match kind {
        TimeOption::Ex | TimeOption::Px => {
            let now = i64::try_from(crate::storage::unix_ms()).unwrap_or(i64::MAX);
            millis.checked_add(now).ok_or_else(invalid)
        }
        TimeOption::ExAt | TimeOption::PxAt => Ok(millis),
    }
}

/// Parse a single RESP command from byte data
///
/// Requests are RESP arrays of bulk strings: `*<count>\r\n$<len>\r\n<data>\r\n...`.
/// Empty requests (`*0\r\n` or a negative count) are skipped, as Redis does.
///
/// # Arguments
/// * `data` - Raw byte slice containing RESP-formatted command
///
/// # Returns
/// * `Ok(Some((consumed_bytes, command)))` - Successfully parsed command
/// * `Ok(None)` - Incomplete data, need more bytes
/// * `Err(...)` - Protocol error or invalid command
pub fn parse_one(data: &[u8]) -> Result<Option<(usize, Cmd)>> {
    let mut consumed = 0;
    loop {
        let rest = &data[consumed..];
        let Some(frame) = read_frame(rest)? else {
            return Ok(None);
        };
        consumed += frame.len;
        let Some((name, items)) = frame.request else {
            continue;
        };
        let cmd = command_from_frame(&rest[name], items).map_err(Refusal::into_error)?;
        return Ok(Some((consumed, cmd)));
    }
}

/// Parse multiple RESP commands from a buffer
///
/// Parses and consumes complete commands until no complete request remains;
/// used for pipelined requests.
///
/// An invalid command (unknown name, wrong number of arguments) is consumed
/// and reported as `Err`; calling again continues with the next request. A
/// protocol error leaves the malformed bytes in `buf`. Use [`parse_requests`]
/// to keep going past invalid commands and tell the two cases apart.
///
/// # Arguments
/// * `buf` - Mutable buffer containing RESP data
/// * `out` - Vector to store parsed commands
pub fn parse_many(buf: &mut BytesMut, out: &mut Vec<Cmd>) -> Result<()> {
    loop {
        let Some(frame) = read_frame(&buf[..])? else {
            return Ok(());
        };
        if let Some((name, items)) = frame.request {
            match command_from_frame(&buf[name], items) {
                Ok(cmd) => out.push(cmd),
                Err(refusal) => {
                    buf.advance(frame.len);
                    return Err(refusal.into_error());
                }
            }
        }
        buf.advance(frame.len);
    }
}

/// One complete request read from a connection buffer
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// A valid command to execute
    Cmd(Cmd),
    /// A well-formed request that is not a valid command (unknown command,
    /// wrong number of arguments, unsupported option). Holds the complete
    /// error line to reply with (see [`write_error`]); the connection stays
    /// usable.
    Invalid(String),
}

/// Parse every complete request in `buf` into `out`, consuming its bytes
///
/// Invalid commands become [`Request::Invalid`] and parsing continues after
/// them. `Err` is returned only for a protocol (framing) error: the requests
/// read before it are already in `out`, and the malformed bytes stay in
/// `buf`. Its message is a complete error line; Redis replies with it and
/// closes the connection.
///
/// A partial request is parsed again from its start on the next call; a
/// server reading from a socket should keep a [`RequestParser`] per
/// connection instead.
pub fn parse_requests(buf: &mut BytesMut, out: &mut Vec<Request>) -> Result<()> {
    RequestParser::new().parse(buf, out)
}

//
// RESP Response Encoders
//
// These functions encode various data types into RESP format for sending
// responses back to clients.
//

/// Encode a simple string response (+OK\r\n)
///
/// Used for status responses like "OK", "PONG", etc.
pub fn resp_simple(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len() + 3);
    v.push(b'+');
    v.extend_from_slice(s.as_bytes());
    v.extend_from_slice(b"\r\n");
    v
}

/// Encode a bulk string response (`$<len>\r\n<data>\r\n`)
///
/// Used for returning string/binary data
pub fn resp_bulk(b: &[u8]) -> Vec<u8> {
    let len_str = b.len().to_string();
    let mut v = Vec::with_capacity(1 + len_str.len() + 2 + b.len() + 2);
    v.push(b'$');
    v.extend_from_slice(len_str.as_bytes());
    v.extend_from_slice(b"\r\n");
    v.extend_from_slice(b);
    v.extend_from_slice(b"\r\n");
    v
}

/// Encode a null response ($-1\r\n)
///
/// Used when a key doesn't exist or operation returns null
pub fn resp_null() -> Vec<u8> {
    b"$-1\r\n".to_vec()
}

/// Encode an integer response (`:<number>\r\n`)
///
/// Used for numeric results like counters, exists checks, etc.
pub fn resp_integer(i: i64) -> Vec<u8> {
    let i_str = i.to_string();
    let mut v = Vec::with_capacity(1 + i_str.len() + 2);
    v.push(b':');
    v.extend_from_slice(i_str.as_bytes());
    v.extend_from_slice(b"\r\n");
    v
}

/// Encode an array response (`*<count>\r\n<item1><item2>...`)
///
/// Used for multi-value responses like MGET results
pub fn resp_array(items: Vec<Vec<u8>>) -> Vec<u8> {
    let len_str = items.len().to_string();
    // Estimate capacity: * + len + \r\n + (items)
    // A rough estimate is better than nothing
    let mut out =
        Vec::with_capacity(1 + len_str.len() + 2 + items.iter().map(|i| i.len()).sum::<usize>());
    out.push(b'*');
    out.extend_from_slice(len_str.as_bytes());
    out.extend_from_slice(b"\r\n");
    for it in items {
        out.extend_from_slice(&it);
    }
    out
}

// Zero-copy writers

/// Write a simple string response (+OK\r\n) directly to buffer
pub fn write_simple(s: &str, out: &mut BytesMut) {
    out.reserve(1 + s.len() + 2);
    out.put_u8(b'+');
    out.put_slice(s.as_bytes());
    out.put_slice(b"\r\n");
}

/// Write an error reply (-ERR ...\r\n) directly to buffer
///
/// `message` is a complete error line such as `ERR no such key`. Line breaks
/// are replaced with spaces so the reply stays a single RESP line.
pub fn write_error(message: &str, out: &mut BytesMut) {
    out.reserve(1 + message.len() + 2);
    out.put_u8(b'-');
    if message.bytes().any(|b| b == b'\r' || b == b'\n') {
        out.extend(message.bytes().map(|b| match b {
            b'\r' | b'\n' => b' ',
            other => other,
        }));
    } else {
        out.put_slice(message.as_bytes());
    }
    out.put_slice(b"\r\n");
}

/// Write a bulk string response (`$<len>\r\n<data>\r\n`) directly to buffer
pub fn write_bulk(b: &[u8], out: &mut BytesMut) {
    let mut digits = [0u8; 20];
    let len = fmt_u64(b.len() as u64, &mut digits);
    out.reserve(1 + len.len() + 2 + b.len() + 2);
    out.put_u8(b'$');
    out.put_slice(len);
    out.put_slice(b"\r\n");
    out.put_slice(b);
    out.put_slice(b"\r\n");
}

/// Write a null response ($-1\r\n) directly to buffer
pub fn write_null(out: &mut BytesMut) {
    out.extend_from_slice(b"$-1\r\n");
}

/// Write the null reply of `protocol`, as for a missing key: `$-1\r\n` in
/// RESP2 (see [`write_null`]) and `_\r\n` in RESP3
pub fn write_nil(protocol: Protocol, out: &mut BytesMut) {
    match protocol {
        Protocol::Resp2 => write_null(out),
        Protocol::Resp3 => out.extend_from_slice(b"_\r\n"),
    }
}

/// Write an integer response (`:<number>\r\n`) directly to buffer
pub fn write_integer(i: i64, out: &mut BytesMut) {
    let mut digits = [0u8; 20];
    let digits = fmt_i64(i, &mut digits);
    out.reserve(1 + digits.len() + 2);
    out.put_u8(b':');
    out.put_slice(digits);
    out.put_slice(b"\r\n");
}

/// Format `n` in decimal into `buf` without allocating and return the digits.
pub(crate) fn fmt_u64(mut n: u64, buf: &mut [u8; 20]) -> &[u8] {
    let mut start = buf.len();
    loop {
        start -= 1;
        buf[start] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    &buf[start..]
}

/// Format `n` in decimal into `buf` without allocating and return the text.
pub(crate) fn fmt_i64(n: i64, buf: &mut [u8; 20]) -> &[u8] {
    // The magnitude of i64::MIN has 19 digits, so the sign always fits.
    let start = buf.len() - fmt_u64(n.unsigned_abs(), buf).len();
    if n < 0 {
        buf[start - 1] = b'-';
        &buf[start - 1..]
    } else {
        &buf[start..]
    }
}

/// Write an aggregate header such as `*<count>\r\n`
fn write_header(kind: u8, n: usize, out: &mut BytesMut) {
    let mut digits = [0u8; 20];
    let digits = fmt_u64(n as u64, &mut digits);
    out.reserve(1 + digits.len() + 2);
    out.put_u8(kind);
    out.put_slice(digits);
    out.put_slice(b"\r\n");
}

/// Write a text reply, as Redis sends INFO: a bulk string in RESP2, and in
/// RESP3 a verbatim string in the `txt` format (`=<len>\r\ntxt:<text>\r\n`)
pub fn write_verbatim(protocol: Protocol, text: &[u8], out: &mut BytesMut) {
    match protocol {
        Protocol::Resp2 => write_bulk(text, out),
        Protocol::Resp3 => {
            write_header(b'=', 4 + text.len(), out);
            out.reserve(4 + text.len() + 2);
            out.put_slice(b"txt:");
            out.put_slice(text);
            out.put_slice(b"\r\n");
        }
    }
}

/// Write array length header (`*<count>\r\n`) directly to buffer
pub fn write_array_len(n: usize, out: &mut BytesMut) {
    write_header(b'*', n, out);
}

/// Write the header of a set of `n` elements: `~<n>\r\n` in RESP3, and in
/// RESP2, which has no sets, the header of an array (as Redis does)
pub fn write_set_len(protocol: Protocol, n: usize, out: &mut BytesMut) {
    match protocol {
        Protocol::Resp2 => write_header(b'*', n, out),
        Protocol::Resp3 => write_header(b'~', n, out),
    }
}

/// Write the header of a map with `n` entries, each written after it as a
/// key and a value: `%<n>\r\n` in RESP3, and in RESP2, which has no maps,
/// the header of an array of `2n` elements (as Redis does)
pub fn write_map_len(protocol: Protocol, n: usize, out: &mut BytesMut) {
    match protocol {
        Protocol::Resp2 => write_header(b'*', 2 * n, out),
        Protocol::Resp3 => write_header(b'%', n, out),
    }
}
