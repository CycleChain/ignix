/*!
 * Redis RESP Protocol Implementation
 *
 * This module implements the Redis Serialization Protocol (RESP) for parsing
 * and encoding commands and responses. It handles the complete protocol specification
 * including command parsing, validation, and response formatting.
 */

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
    /// PING [message] - test connectivity; echoes `message` when given
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
    /// MGET key1 key2 ... - get multiple keys
    MGet(Vec<Bytes>),
    /// MSET key1 value1 key2 value2 ... - set multiple key-value pairs
    MSet(Vec<(Bytes, Bytes)>),
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
    let Some(&first) = data.first() else {
        return Ok(None);
    };
    if first != b'*' {
        bail!(
            "ERR Protocol error: expected '*', got '{}'",
            first.escape_ascii()
        );
    }
    let Some((mut cursor, count)) = read_int_line(data, 1, INVALID_MULTIBULK)? else {
        return Ok(None);
    };
    if count > MAX_MULTIBULK_LEN {
        bail!(INVALID_MULTIBULK);
    }
    if count <= 0 {
        return Ok(Some((cursor, Vec::new())));
    }
    let count = count as usize;

    // Do not trust the announced count for preallocation: every element needs
    // at least MIN_ELEMENT_LEN bytes of input that must actually arrive.
    let mut items = Vec::with_capacity(count.min((data.len() - cursor) / MIN_ELEMENT_LEN + 1));
    for _ in 0..count {
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
        items.push(Bytes::copy_from_slice(&data[start..end]));
        cursor = end + 2;
    }
    Ok(Some((cursor, items)))
}

/// Commands known to the parser, with the name Redis uses in error messages.
#[derive(Clone, Copy)]
enum Kind {
    Ping,
    Get,
    Set,
    Del,
    Rename,
    Exists,
    Incr,
    MGet,
    MSet,
}

const COMMANDS: [(&str, Kind); 9] = [
    ("ping", Kind::Ping),
    ("get", Kind::Get),
    ("set", Kind::Set),
    ("del", Kind::Del),
    ("rename", Kind::Rename),
    ("exists", Kind::Exists),
    ("incr", Kind::Incr),
    ("mget", Kind::MGet),
    ("mset", Kind::MSet),
];

/// SET options that Redis supports but Ignix does not implement yet.
const UNSUPPORTED_SET_OPTIONS: [&str; 8] =
    ["NX", "XX", "GET", "EX", "PX", "EXAT", "PXAT", "KEEPTTL"];

/// Build a command from the arguments of one request frame.
///
/// On failure returns a complete RESP error line worded like Redis 7.
fn command_from_frame(mut items: Vec<Bytes>) -> std::result::Result<Cmd, String> {
    let Some(&(name, kind)) = COMMANDS
        .iter()
        .find(|(name, _)| items[0].eq_ignore_ascii_case(name.as_bytes()))
    else {
        return Err(unknown_command_error(&items));
    };

    let argc = items.len();
    let arity_ok = match kind {
        Kind::Ping => argc <= 2,
        Kind::Get | Kind::Incr => argc == 2,
        Kind::Rename => argc == 3,
        Kind::Set => argc >= 3,
        Kind::Del | Kind::Exists | Kind::MGet => argc >= 2,
        Kind::MSet => argc >= 3 && argc % 2 == 1,
    };
    let arity_error = || format!("ERR wrong number of arguments for '{name}' command");
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
        Kind::Get | Kind::Incr => {
            let [key] = <[Bytes; 1]>::try_from(items).map_err(|_| arity_error())?;
            if matches!(kind, Kind::Get) {
                Cmd::Get(key)
            } else {
                Cmd::Incr(key)
            }
        }
        Kind::Set => {
            let [key, value] = <[Bytes; 2]>::try_from(items).map_err(|_| arity_error())?;
            Cmd::Set(key, value)
        }
        Kind::Rename => {
            let [from, to] = <[Bytes; 2]>::try_from(items).map_err(|_| arity_error())?;
            Cmd::Rename(from, to)
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
/// This function continuously parses commands from the buffer until
/// no complete commands remain. It's used for handling pipelined requests.
///
/// # Arguments
/// * `buf` - Mutable buffer containing RESP data
/// * `out` - Vector to store parsed commands
pub fn parse_many(buf: &mut bytes::BytesMut, out: &mut Vec<Cmd>) -> Result<()> {
    loop {
        let (consumed, cmd) = match parse_one(&buf[..])? {
            Some(x) => x,
            None => break, // No complete command available
        };

        // Remove consumed bytes from buffer
        buf.advance(consumed);
        out.push(cmd);
    }
    Ok(())
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

/// Encode a bulk string response ($<len>\r\n<data>\r\n)
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

/// Encode an integer response (:<number>\r\n)
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

/// Encode an array response (*<count>\r\n<item1><item2>...)
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

/// Write a bulk string response ($<len>\r\n<data>\r\n) directly to buffer
pub fn write_bulk(b: &[u8], out: &mut BytesMut) {
    let len_str = b.len().to_string();
    out.reserve(1 + len_str.len() + 2 + b.len() + 2);
    out.put_u8(b'$');
    out.put_slice(len_str.as_bytes());
    out.put_slice(b"\r\n");
    out.put_slice(b);
    out.put_slice(b"\r\n");
}

/// Write a null response ($-1\r\n) directly to buffer
pub fn write_null(out: &mut BytesMut) {
    out.extend_from_slice(b"$-1\r\n");
}

/// Write an integer response (:<number>\r\n) directly to buffer
pub fn write_integer(i: i64, out: &mut BytesMut) {
    let i_str = i.to_string();
    out.reserve(1 + i_str.len() + 2);
    out.put_u8(b':');
    out.put_slice(i_str.as_bytes());
    out.put_slice(b"\r\n");
}

/// Write array length header (*<count>\r\n) directly to buffer
pub fn write_array_len(n: usize, out: &mut BytesMut) {
    let len_str = n.to_string();
    out.reserve(1 + len_str.len() + 2);
    out.put_u8(b'*');
    out.put_slice(len_str.as_bytes());
    out.put_slice(b"\r\n");
}
