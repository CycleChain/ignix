/*!
 * Redis RESP Protocol Implementation
 * 
 * This module implements the Redis Serialization Protocol (RESP) for parsing
 * and encoding commands and responses. It handles the complete protocol specification
 * including command parsing, validation, and response formatting.
 */

use anyhow::*;
use bytes::{Buf, Bytes};

/// Redis-compatible commands supported by Ignix
/// 
/// Each variant represents a specific Redis command with its parameters.
/// All data is stored as byte vectors to handle both text and binary data.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    /// PING command - test server connectivity
    Ping,
    /// GET key - retrieve value for a key
    Get(Bytes),
    /// SET key value - set a key-value pair
    Set(Bytes, Bytes),
    /// DEL key - delete a key
    Del(Bytes),
    /// RENAME oldkey newkey - rename a key
    Rename(Bytes, Bytes),
    /// EXISTS key - check if key exists
    Exists(Bytes),
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
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// String/binary data
    Str(Bytes),
    /// 64-bit signed integer
    Int(i64),
    /// Binary blob (same as Str but semantically different)
    Blob(Bytes),
}

/// Parse a single RESP command from byte data
/// 
/// This function implements the core RESP parsing logic according to the Redis protocol.
/// It expects commands in the format: *<count>\r\n$<len>\r\n<data>\r\n...
/// 
/// # Arguments
/// * `data` - Raw byte slice containing RESP-formatted command
/// 
/// # Returns
/// * `Ok(Some((consumed_bytes, command)))` - Successfully parsed command
/// * `Ok(None)` - Incomplete data, need more bytes
/// * `Err(...)` - Protocol error or invalid command
pub fn parse_one(data: &[u8]) -> Result<Option<(usize, Cmd)>> {
    // Check if we have any data to parse
    if data.is_empty() {
        return Ok(None);
    }
    
    // RESP arrays must start with '*'
    if data[0] != b'*' {
        bail!("protocol error: expected array");
    }
    
    // Read the number of array elements
    let (i, n) = read_decimal_line(&data[1..])?;
    if i == 0 {
        return Ok(None);
    }
    let mut cursor = 1 + i;
    
    if n <= 0 {
        bail!("empty array");
    }
    
    // Pre-allocate vector for better performance
    let mut items: Vec<Bytes> = Vec::with_capacity(n as usize);
    
    // Parse each array element (bulk strings)
    for _ in 0..n {
        // Check if we have enough data
        if cursor >= data.len() {
            return Ok(None); // Need more data
        }
        
        // Each element must be a bulk string starting with '$'
        if data[cursor] != b'$' {
            bail!("expected bulk");
        }
        
        // Read the length of this bulk string
        let (i2, len) = read_decimal_line(&data[cursor + 1..])?;
        if i2 == 0 {
            return Ok(None);
        }
        cursor += 1 + i2;
        
        // Calculate total bytes needed (length + \r\n)
        let need = len as usize + 2;
        if cursor + need > data.len() {
            return Ok(None); // Need more data
        }
        
        // Extract the payload
        let payload = &data[cursor..cursor + len as usize];
        items.push(Bytes::copy_from_slice(payload));
        cursor += need;
    }
    
    if items.is_empty() {
        bail!("empty array body");
    }
    
    // Match command names and validate argument counts
    // Using case-insensitive comparison without allocation
    let cmd = if items[0].eq_ignore_ascii_case(b"PING") {
        Cmd::Ping
    } else if items[0].eq_ignore_ascii_case(b"GET") && items.len() >= 2 {
        Cmd::Get(items[1].clone())
    } else if items[0].eq_ignore_ascii_case(b"SET") && items.len() >= 3 {
        Cmd::Set(items[1].clone(), items[2].clone())
    } else if items[0].eq_ignore_ascii_case(b"DEL") && items.len() >= 2 {
        Cmd::Del(items[1].clone())
    } else if items[0].eq_ignore_ascii_case(b"RENAME") && items.len() >= 3 {
        Cmd::Rename(items[1].clone(), items[2].clone())
    } else if items[0].eq_ignore_ascii_case(b"EXISTS") && items.len() >= 2 {
        Cmd::Exists(items[1].clone())
    } else if items[0].eq_ignore_ascii_case(b"INCR") && items.len() >= 2 {
        Cmd::Incr(items[1].clone())
    } else if items[0].eq_ignore_ascii_case(b"MGET") && items.len() >= 2 {
        Cmd::MGet(items[1..].to_vec())
    } else if items[0].eq_ignore_ascii_case(b"MSET") && items.len() >= 3 && items.len() % 2 == 1 {
        // MSET requires odd number of args (command + key-value pairs)
        let mut v = Vec::with_capacity((items.len() - 1) / 2);
        for pair in items[1..].chunks(2) {
            if pair.len() == 2 {
                v.push((pair[0].clone(), pair[1].clone()));
            }
        }
        Cmd::MSet(v)
    } else {
        bail!("unknown/invalid command");
    };
    
    Ok(Some((cursor, cmd)))
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

/// Read a decimal number followed by \r\n
/// 
/// Helper function to parse RESP numeric fields like array lengths
/// and bulk string lengths.
/// 
/// # Returns
/// * `(bytes_consumed, parsed_number)`
fn read_decimal_line(s: &[u8]) -> Result<(usize, i64)> {
    let mut i = 0;
    let mut num: i64 = 0;
    let mut sign: i64 = 1;

    if i < s.len() && s[i] == b'-' {
        sign = -1;
        i += 1;
    }

    let start = i;

    // Fast path: Try to read 8 bytes at a time
    // We only do this if we have at least 8 bytes remaining
    while i + 8 <= s.len() {
        // Read 8 bytes as u64 (Little Endian)
        let chunk = u64::from_le_bytes(s[i..i+8].try_into().unwrap());
        
        // Check if all bytes are digits (0x30..=0x39)
        // Algorithm:
        // 1. Add 0x46 (0x7F - 0x39) to each byte. If byte was > 0x39, it will overflow into high bit (0x80).
        // 2. Subtract 0x30 from each byte. If byte was < 0x30, it will underflow (borrow) from high bit?
        // Actually, a simpler check for "is any byte not a digit":
        // val < '0' || val > '9'
        // Using SWAR:
        // has_less = (chunk - 0x3030303030303030) & 0x8080808080808080
        // has_more = (chunk + 0x4646464646464646) & 0x8080808080808080
        // If either is non-zero, we have a non-digit.
        
        let val_minus_0 = chunk.wrapping_sub(0x3030303030303030);
        let val_plus_46 = chunk.wrapping_add(0x4646464646464646); // 0x46 = 127 - 57 ('9')
        
        if (val_minus_0 | val_plus_46) & 0x8080808080808080 != 0 {
            // Found a non-digit in this chunk, fall back to byte-by-byte
            break;
        }

        // All 8 bytes are digits. Parse them.
        // This is tricky to do efficiently in parallel without SIMD intrinsics.
        // But we can just loop unroll here since we know they are valid.
        // Or just continue byte-by-byte loop which is now branch-predicted well?
        // Actually, the main win of SWAR is finding the *end* of the string quickly.
        // Parsing still needs multiplication.
        // Let's just fall through to byte loop, but we know the next 8 bytes are valid?
        // No, let's just use the byte loop. The "SWAR" part here is mostly useful for *skipping* or *finding* delimiters.
        // For parsing integers, the multiplication dependency chain is the bottleneck.
        
        // Let's stick to a simple unrolled loop which compilers vectorize well.
        break; 
    }

    // Standard loop (compiler usually vectorizes this well if simple)
    while i < s.len() {
        let c = s[i];
        if c.is_ascii_digit() {
            num = num.wrapping_mul(10).wrapping_add((c - b'0') as i64);
            i += 1;
        } else {
            break;
        }
    }

    if i == start {
        // No digits found
    }

    // Check for \r\n
    if i + 1 < s.len() && s[i] == b'\r' && s[i + 1] == b'\n' {
        Ok((i + 2, num * sign))
    } else if i + 1 >= s.len() {
        // Incomplete
        Ok((0, 0))
    } else {
        bail!("expected CRLF");
    }
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
    let mut out = Vec::with_capacity(1 + len_str.len() + 2 + items.iter().map(|i| i.len()).sum::<usize>());
    out.push(b'*');
    out.extend_from_slice(len_str.as_bytes());
    out.extend_from_slice(b"\r\n");
    for it in items {
        out.extend_from_slice(&it);
    }
    out
}