//! Request parsing and reply writing APIs used by the network backends.

use bytes::{Bytes, BytesMut};
use ignix::protocol::{parse_one, parse_requests, write_error, Request};
use ignix::Cmd;

fn b(s: &str) -> Bytes {
    Bytes::copy_from_slice(s.as_bytes())
}

#[test]
fn parse_requests_reports_invalid_commands_and_continues() {
    let mut buf = BytesMut::from(
        &b"*3\r\n$3\r\nSET\r\n$1\r\na\r\n$1\r\n1\r\n*1\r\n$3\r\nFOO\r\n*2\r\n$3\r\nGET\r\n$1\r\na\r\n"[..],
    );
    let mut out = Vec::new();
    parse_requests(&mut buf, &mut out).unwrap();
    assert_eq!(
        out,
        vec![
            Request::Cmd(Cmd::Set(b("a"), b("1"))),
            Request::Invalid("ERR unknown command 'FOO', with args beginning with: ".to_string()),
            Request::Cmd(Cmd::Get(b("a"))),
        ]
    );
    assert!(buf.is_empty());
}

#[test]
fn parse_requests_keeps_requests_before_protocol_error() {
    let mut buf = BytesMut::from(&b"*1\r\n$4\r\nPING\r\n*1\r\nX"[..]);
    let mut out = Vec::new();
    let err = parse_requests(&mut buf, &mut out).unwrap_err();
    assert_eq!(err.to_string(), "ERR Protocol error: expected '$', got 'X'");
    assert_eq!(out, vec![Request::Cmd(Cmd::Ping(None))]);
    assert_eq!(&buf[..], b"*1\r\nX");
}

#[test]
fn parse_requests_leaves_incomplete_frame_in_buffer() {
    let mut buf = BytesMut::from(&b"*1\r\n$4\r\nPING\r\n*2\r\n$3\r\nGET"[..]);
    let mut out = Vec::new();
    parse_requests(&mut buf, &mut out).unwrap();
    assert_eq!(out, vec![Request::Cmd(Cmd::Ping(None))]);
    assert_eq!(&buf[..], b"*2\r\n$3\r\nGET");
}

#[test]
fn parse_requests_skips_empty_requests() {
    let mut buf = BytesMut::from(&b"*0\r\n*-1\r\n*1\r\n$4\r\nPING\r\n"[..]);
    let mut out = Vec::new();
    parse_requests(&mut buf, &mut out).unwrap();
    assert_eq!(out, vec![Request::Cmd(Cmd::Ping(None))]);
    assert!(buf.is_empty());
}

#[test]
fn write_error_writes_a_resp_error_line() {
    let mut out = BytesMut::new();
    write_error("ERR no such key", &mut out);
    assert_eq!(&out[..], b"-ERR no such key\r\n");
}

#[test]
fn write_error_replaces_line_breaks() {
    let mut out = BytesMut::new();
    write_error("ERR a\r\nb", &mut out);
    assert_eq!(&out[..], b"-ERR a  b\r\n");
}

#[test]
fn commands_have_the_expected_shapes() {
    let parse = |data: &[u8]| parse_one(data).unwrap().unwrap().1;
    assert_eq!(parse(b"*1\r\n$4\r\nPING\r\n"), Cmd::Ping(None));
    assert_eq!(
        parse(b"*2\r\n$4\r\nPING\r\n$2\r\nhi\r\n"),
        Cmd::Ping(Some(b("hi")))
    );
    assert_eq!(
        parse(b"*3\r\n$3\r\nDEL\r\n$1\r\na\r\n$1\r\nb\r\n"),
        Cmd::Del(vec![b("a"), b("b")])
    );
    assert_eq!(
        parse(b"*2\r\n$6\r\nEXISTS\r\n$1\r\na\r\n"),
        Cmd::Exists(vec![b("a")])
    );
    assert_eq!(
        parse(b"*5\r\n$4\r\nMSET\r\n$1\r\na\r\n$1\r\n1\r\n$1\r\nb\r\n$1\r\n2\r\n"),
        Cmd::MSet(vec![(b("a"), b("1")), (b("b"), b("2"))])
    );
}

#[test]
fn dict_incr_reports_redis_errors_without_changing_the_value() {
    let dict = ignix::Dict::default();
    dict.set(b("text"), ignix::Value::Str(b("abc")));
    assert_eq!(dict.incr(b("text")), Err(ignix::IncrError::NotAnInteger));
    assert_eq!(dict.get(b"text"), Some(ignix::Value::Str(b("abc"))));

    dict.set(b("max"), ignix::Value::Int(i64::MAX));
    assert_eq!(dict.incr(b("max")), Err(ignix::IncrError::Overflow));
    assert_eq!(dict.get(b"max"), Some(ignix::Value::Int(i64::MAX)));

    assert_eq!(dict.incr(b("new")), Ok(1));
    assert_eq!(
        ignix::IncrError::NotAnInteger.to_string(),
        "ERR value is not an integer or out of range"
    );
    assert_eq!(
        ignix::IncrError::Overflow.to_string(),
        "ERR increment or decrement would overflow"
    );
}

#[test]
fn emit_aof_del_encodes_every_key() {
    assert_eq!(
        ignix::emit_aof_del(&[b("a"), Bytes::from_static(b"\xff")]),
        b"*3\r\n$3\r\nDEL\r\n$1\r\na\r\n$1\r\n\xff\r\n"
    );
}
