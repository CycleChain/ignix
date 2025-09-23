/*!
 * Redis RESP Protocol Implementation
 * 
 * This module implements the Redis Serialization Protocol (RESP) for parsing
 * and encoding commands and responses. It handles the complete protocol specification
 * including command parsing, validation, and response formatting.
 */

use anyhow::*;
use bytes::Buf;

/// Redis-compatible commands supported by Ignix
/// 
/// Each variant represents a specific Redis command with its parameters.
/// All data is stored as byte vectors to handle both text and binary data.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    /// PING command - test server connectivity
    Ping,
    /// GET key - retrieve value for a key
    Get(Vec<u8>),
    /// SET key value - set a key-value pair
    Set(Vec<u8>, Vec<u8>),
    /// DEL key - delete a key
    Del(Vec<u8>),
    /// RENAME oldkey newkey - rename a key
    Rename(Vec<u8>, Vec<u8>),
    /// EXISTS key - check if key exists
    Exists(Vec<u8>),
    /// INCR key - increment numeric value
    Incr(Vec<u8>),
    /// MGET key1 key2 ... - get multiple keys
    MGet(Vec<Vec<u8>>),
    /// MSET key1 value1 key2 value2 ... - set multiple key-value pairs
    MSet(Vec<(Vec<u8>, Vec<u8>)>),
}

/// Value types that can be stored in Ignix
/// 
/// Supports different data types while maintaining Redis compatibility.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// String/binary data
    Str(Vec<u8>),
    /// 64-bit signed integer
    Int(i64),
    /// Binary blob (same as Str but semantically different)
    Blob(Vec<u8>),
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
    let mut cursor = 1 + i;
    
    if n <= 0 {
        bail!("empty array");
    }
    
    // Pre-allocate vector for better performance
    let mut items: Vec<Vec<u8>> = Vec::with_capacity(n as usize);
    
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
        cursor += 1 + i2;
        
        // Calculate total bytes needed (length + \r\n)
        let need = len as usize + 2;
        if cursor + need > data.len() {
            return Ok(None); // Need more data
        }
        
        // Extract the payload
        let payload = &data[cursor..cursor + len as usize];
        items.push(payload.to_vec());
        cursor += need;
    }
    
    if items.is_empty() {
        bail!("empty array body");
    }
    
    // Convert command name to uppercase for case-insensitive matching
    let cmd_name = upper_ascii(&items[0]);
    
    // Match command names and validate argument counts
    let cmd = match &cmd_name[..] {
        b"PING" => Cmd::Ping,
        b"GET" if items.len() >= 2 => Cmd::Get(items[1].clone()),
        b"SET" if items.len() >= 3 => Cmd::Set(items[1].clone(), items[2].clone()),
        b"DEL" if items.len() >= 2 => Cmd::Del(items[1].clone()),
        b"RENAME" if items.len() >= 3 => Cmd::Rename(items[1].clone(), items[2].clone()),
        b"EXISTS" if items.len() >= 2 => Cmd::Exists(items[1].clone()),
        b"INCR" if items.len() >= 2 => Cmd::Incr(items[1].clone()),
        b"MGET" if items.len() >= 2 => Cmd::MGet(items[1..].to_vec()),
        b"MSET" if items.len() >= 3 && items.len() % 2 == 1 => {
            // MSET requires odd number of args (command + key-value pairs)
            let mut v = Vec::new();
            for pair in items[1..].chunks(2) {
                if pair.len() == 2 {
                    v.push((pair[0].clone(), pair[1].clone()));
                }
            }
            Cmd::MSet(v)
        }
        _ => {
            bail!("unknown/invalid command");
        }
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
    
    // Find the end of the line (\r\n)
    while i + 1 < s.len() && !(s[i] == b'\r' && s[i + 1] == b'\n') {
        i += 1;
    }
    
    // Check if we found a complete line
    if i + 1 >= s.len() {
        return Ok((0, 0)); // Incomplete line
    }
    
    // Parse the number
    let num = std::str::from_utf8(&s[..i])?.parse::<i64>()?;
    Ok((i + 2, num)) // +2 for \r\n
}

/// Convert byte slice to uppercase ASCII
/// 
/// Used for case-insensitive command matching
#[inline]
fn upper_ascii(s: &[u8]) -> Vec<u8> {
    s.iter().map(|c| c.to_ascii_uppercase()).collect()
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
    format!("+{}\r\n", s).into_bytes()
}

/// Encode a bulk string response ($<len>\r\n<data>\r\n)
/// 
/// Used for returning string/binary data
pub fn resp_bulk(b: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(format!("${}\r\n", b.len()).as_bytes());
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
    format!(":{}\r\n", i).into_bytes()
}

/// Encode an array response (*<count>\r\n<item1><item2>...)
/// 
/// Used for multi-value responses like MGET results
pub fn resp_array(items: Vec<Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("*{}\r\n", items.len()).as_bytes());
    for it in items {
        out.extend_from_slice(&it);
    }
    out
}