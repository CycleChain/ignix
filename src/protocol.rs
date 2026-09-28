/*!
 * Redis RESP Protocol Implementation
 *
 * This module implements the Redis Serialization Protocol (RESP) for parsing
 * and encoding commands and responses. It handles the complete protocol specification
 * including command parsing, validation, and response formatting.
 */

use crate::commands::{self, Kind};
use anyhow::{bail, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};

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

/// Redis-compatible commands supported by Ignix
///
/// Each variant represents a specific Redis command with its parameters.
/// Keys and values are `Bytes`, so both text and binary data are supported.
/// More commands will be added, so matches outside this crate need a
/// wildcard arm.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
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

/// Read one complete request frame: `*<n>\r\n` followed by `n` bulk strings.
///
/// Returns `Ok(None)` when more data is needed and the consumed length with
/// the arguments otherwise. Malformed input is an error whose message is a
/// complete RESP error line (`ERR Protocol error: ...`). A frame with `n <= 0`
/// has no arguments; Redis ignores such requests.
fn read_frame(data: &[u8]) -> Result<Option<(usize, Vec<Bytes>)>> {
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
    /// Elements of the current request read so far
    items: Vec<Bytes>,
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
        loop {
            let Some((consumed, items)) = self.next_frame(&buf[..])? else {
                return Ok(());
            };
            buf.advance(consumed);
            if items.is_empty() {
                continue;
            }
            out.push(match command_from_frame(items) {
                Ok(cmd) => Request::Cmd(cmd),
                Err(message) => Request::Invalid(message),
            });
        }
    }

    /// Forget the request in progress
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Continue reading the frame at the start of `data`; see [`read_frame`].
    fn next_frame(&mut self, data: &[u8]) -> Result<Option<(usize, Vec<Bytes>)>> {
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
                return Ok(Some((cursor, Vec::new())));
            }
            let count = count as usize;
            // Do not trust the announced count for preallocation: every
            // element needs at least MIN_ELEMENT_LEN bytes of input that must
            // actually arrive.
            self.items = Vec::with_capacity(count.min((data.len() - cursor) / MIN_ELEMENT_LEN + 1));
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
            self.items.push(Bytes::copy_from_slice(&data[start..end]));
            self.cursor = end + 2;
            self.remaining -= 1;
        }
        let consumed = std::mem::take(&mut self.cursor);
        Ok(Some((consumed, std::mem::take(&mut self.items))))
    }
}

/// SET options that Redis supports but Ignix does not implement yet.
const UNSUPPORTED_SET_OPTIONS: [&str; 8] =
    ["NX", "XX", "GET", "EX", "PX", "EXAT", "PXAT", "KEEPTTL"];

/// Build a command from the arguments of one request frame.
///
/// On failure returns a complete RESP error line worded like Redis 7.
fn command_from_frame(mut items: Vec<Bytes>) -> std::result::Result<Cmd, String> {
    let Some(spec) = commands::lookup(&items[0]) else {
        return Err(unknown_command_error(&items));
    };
    let kind = spec.kind;

    let argc = items.len();
    let arity_error = || format!("ERR wrong number of arguments for '{}' command", spec.name);
    if !spec.arity_matches(argc) {
        return Err(arity_error());
    }
    // Limits Redis checks in the commands themselves, with the same error
    let arity_ok = match kind {
        Kind::Ping => argc <= 2,
        Kind::MSet => argc % 2 == 1,
        _ => true,
    };
    if !arity_ok {
        return Err(arity_error());
    }
    if matches!(kind, Kind::Set) && argc > 3 {
        return Err(set_option_error(&items[3]));
    }

    // Drop the command name; what is left are the arguments.
    items.remove(0);
    let cmd = match kind {
        Kind::Ping => Cmd::Ping(items.pop()),
        Kind::Del => Cmd::Del(items),
        Kind::Exists => Cmd::Exists(items),
        Kind::MGet => Cmd::MGet(items),
        Kind::MSet => {
            let mut pairs = Vec::with_capacity(items.len() / 2);
            let mut args = items.into_iter();
            while let (Some(key), Some(value)) = (args.next(), args.next()) {
                pairs.push((key, value));
            }
            Cmd::MSet(pairs)
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
        Kind::Set => {
            let [key, value] = <[Bytes; 2]>::try_from(items).map_err(|_| arity_error())?;
            Cmd::Set(key, value)
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

/// `ERR unknown command ...`, truncated like Redis: the name is cut to 128
/// bytes, and quoted arguments are appended while that part is shorter than
/// 128 bytes (each one cut to the remaining room).
fn unknown_command_error(items: &[Bytes]) -> String {
    const LIMIT: usize = 128;
    let name = &items[0];
    let mut msg = Vec::with_capacity(96);
    msg.extend_from_slice(b"ERR unknown command '");
    msg.extend_from_slice(&name[..name.len().min(LIMIT)]);
    msg.extend_from_slice(b"', with args beginning with: ");
    let args_start = msg.len();
    for arg in &items[1..] {
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

/// Error for the first extra `SET` argument: options Redis knows are reported
/// as unsupported, anything else is a syntax error (as in Redis).
fn set_option_error(option: &[u8]) -> String {
    match UNSUPPORTED_SET_OPTIONS
        .iter()
        .find(|name| option.eq_ignore_ascii_case(name.as_bytes()))
    {
        Some(name) => format!("ERR SET option '{name}' is not supported"),
        None => "ERR syntax error".to_string(),
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
        let Some((len, items)) = read_frame(&data[consumed..])? else {
            return Ok(None);
        };
        consumed += len;
        if items.is_empty() {
            continue;
        }
        let cmd = command_from_frame(items).map_err(anyhow::Error::msg)?;
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
        let Some((consumed, items)) = read_frame(&buf[..])? else {
            return Ok(());
        };
        buf.advance(consumed);
        if items.is_empty() {
            continue;
        }
        out.push(command_from_frame(items).map_err(anyhow::Error::msg)?);
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

/// Write array length header (`*<count>\r\n`) directly to buffer
pub fn write_array_len(n: usize, out: &mut BytesMut) {
    let mut digits = [0u8; 20];
    let digits = fmt_u64(n as u64, &mut digits);
    out.reserve(1 + digits.len() + 2);
    out.put_u8(b'*');
    out.put_slice(digits);
    out.put_slice(b"\r\n");
}
