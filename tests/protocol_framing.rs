//! RESP framing: malformed lengths must become protocol errors instead of
//! panics, and incomplete input must keep waiting for more data.

use bytes::BytesMut;
use ignix::protocol::{parse_many, parse_one};

/// Parse `data` with `parse_one` and return the protocol error message.
fn protocol_error(data: &[u8]) -> String {
    match parse_one(data) {
        Err(e) => e.to_string(),
        Ok(parsed) => panic!(
            "expected a protocol error for {:?}, got {:?}",
            String::from_utf8_lossy(data),
            parsed
        ),
    }
}

#[test]
fn negative_bulk_length_is_a_protocol_error() {
    let err = protocol_error(b"*1\r\n$-5\r\n");
    assert_eq!(err, "ERR Protocol error: invalid bulk length");
}

#[test]
fn bulk_length_minus_one_is_a_protocol_error() {
    let err = protocol_error(b"*1\r\n$-1\r\nX");
    assert_eq!(err, "ERR Protocol error: invalid bulk length");
}

#[test]
fn multibulk_length_near_i64_max_is_rejected_without_allocating() {
    let err = protocol_error(b"*9223372036854775807\r\n");
    assert_eq!(err, "ERR Protocol error: invalid multibulk length");
}

#[test]
fn bulk_length_above_512_mib_is_rejected() {
    let err = protocol_error(b"*1\r\n$536870913\r\n");
    assert_eq!(err, "ERR Protocol error: invalid bulk length");
}

#[test]
fn bulk_length_of_512_mib_is_accepted_and_waits_for_data() {
    assert!(matches!(parse_one(b"*1\r\n$536870912\r\n"), Ok(None)));
}

#[test]
fn overlong_length_line_is_rejected() {
    // 20 digits: does not fit in an i64.
    let err = protocol_error(b"*1\r\n$99999999999999999999\r\n");
    assert_eq!(err, "ERR Protocol error: invalid bulk length");
    // 21 characters: longer than any valid length line.
    let err = protocol_error(b"*1\r\n$123456789012345678901\r\n");
    assert_eq!(err, "ERR Protocol error: invalid bulk length");
}

#[test]
fn length_line_without_terminator_is_rejected_once_too_long() {
    let err = protocol_error(b"*1\r\n$1234567890123456789012345");
    assert_eq!(err, "ERR Protocol error: invalid bulk length");
}

#[test]
fn length_with_leading_zero_is_rejected() {
    let err = protocol_error(b"*01\r\n$4\r\nPING\r\n");
    assert_eq!(err, "ERR Protocol error: invalid multibulk length");
    let err = protocol_error(b"*1\r\n$04\r\nPING\r\n");
    assert_eq!(err, "ERR Protocol error: invalid bulk length");
}

#[test]
fn length_with_plus_sign_is_rejected() {
    let err = protocol_error(b"*1\r\n$+4\r\nPING\r\n");
    assert_eq!(err, "ERR Protocol error: invalid bulk length");
}

#[test]
fn bulk_payload_must_be_followed_by_crlf() {
    let err = protocol_error(b"*1\r\n$4\r\nPINGxx");
    assert_eq!(err, "ERR Protocol error: invalid bulk length");
}

#[test]
fn element_without_dollar_prefix_is_rejected() {
    let err = protocol_error(b"*1\r\n:4\r\n");
    assert_eq!(err, "ERR Protocol error: expected '$', got ':'");
}

#[test]
fn request_without_array_prefix_is_rejected() {
    let err = protocol_error(b"PING\r\n");
    assert_eq!(err, "ERR Protocol error: expected '*', got 'P'");
}

#[test]
fn empty_multibulk_is_ignored_like_redis() {
    let mut buf = BytesMut::from(&b"*0\r\n*1\r\n$4\r\nPING\r\n"[..]);
    let mut cmds = Vec::new();
    parse_many(&mut buf, &mut cmds).expect("empty multibulk must be skipped");
    assert_eq!(cmds.len(), 1);
    assert!(format!("{:?}", cmds[0]).starts_with("Ping"));
    assert!(buf.is_empty());
}

#[test]
fn negative_multibulk_is_ignored_like_redis() {
    let mut buf = BytesMut::from(&b"*-1\r\n*1\r\n$4\r\nPING\r\n"[..]);
    let mut cmds = Vec::new();
    parse_many(&mut buf, &mut cmds).expect("negative multibulk must be skipped");
    assert_eq!(cmds.len(), 1);
    assert!(buf.is_empty());
}

#[test]
fn incomplete_frames_wait_for_more_data() {
    let frame: &[u8] = b"*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$5\r\nhello\r\n";
    for end in 0..frame.len() {
        assert!(
            matches!(parse_one(&frame[..end]), Ok(None)),
            "prefix {:?} must wait for more data",
            String::from_utf8_lossy(&frame[..end])
        );
    }
    let (consumed, _) = parse_one(frame).unwrap().unwrap();
    assert_eq!(consumed, frame.len());
}

#[test]
fn protocol_error_leaves_buffer_untouched() {
    let mut buf = BytesMut::from(&b"*1\r\n$4\r\nPING\r\n*1\r\nX"[..]);
    let mut cmds = Vec::new();
    assert!(parse_many(&mut buf, &mut cmds).is_err());
    assert_eq!(cmds.len(), 1, "the PING before the bad frame is parsed");
    assert_eq!(&buf[..], b"*1\r\nX");
}

#[test]
fn parse_many_consumes_an_invalid_command_before_reporting_it() {
    let mut buf = BytesMut::from(&b"*1\r\n$3\r\nFOO\r\n*1\r\n$4\r\nPING\r\n"[..]);
    let mut cmds = Vec::new();
    assert!(parse_many(&mut buf, &mut cmds).is_err());
    assert_eq!(
        &buf[..],
        b"*1\r\n$4\r\nPING\r\n",
        "the invalid command must not stay at the head of the buffer"
    );
    parse_many(&mut buf, &mut cmds).unwrap();
    assert_eq!(cmds.len(), 1);
    assert!(buf.is_empty());
}
