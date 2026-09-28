//! Request parsing and reply writing APIs used by the network backends.

use bytes::{Bytes, BytesMut};
use ignix::protocol::{parse_one, parse_requests, write_error, Request, RequestParser};
use ignix::Cmd;
use std::time::{Duration, Instant};

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

#[test]
fn integer_replies_cover_the_whole_i64_range() {
    use ignix::protocol::{write_array_len, write_bulk, write_integer};
    let mut out = BytesMut::new();
    for (value, expected) in [
        (0i64, &b":0\r\n"[..]),
        (-1, b":-1\r\n"),
        (i64::MAX, b":9223372036854775807\r\n"),
        (i64::MIN, b":-9223372036854775808\r\n"),
    ] {
        out.clear();
        write_integer(value, &mut out);
        assert_eq!(&out[..], expected);
    }
    out.clear();
    write_array_len(0, &mut out);
    write_array_len(1234567, &mut out);
    write_bulk(b"", &mut out);
    write_bulk(&[b'x'; 12], &mut out);
    assert_eq!(
        &out[..],
        b"*0\r\n*1234567\r\n$0\r\n\r\n$12\r\nxxxxxxxxxxxx\r\n"
    );
}

/// RESP encoding of a request
fn request(args: &[&[u8]]) -> Vec<u8> {
    let mut out = format!("*{}\r\n", args.len()).into_bytes();
    for arg in args {
        out.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
        out.extend_from_slice(arg);
        out.extend_from_slice(b"\r\n");
    }
    out
}

#[test]
fn request_parser_gives_the_same_requests_when_fed_one_byte_at_a_time() {
    let mut input = request(&[b"SET", b"a", b"1"]);
    input.extend(request(&[b"FOO", b"bar"]));
    input.extend_from_slice(b"*0\r\n");
    input.extend(request(&[b"GET", b"a"]));
    let mut mset: Vec<&[u8]> = vec![b"MSET"];
    let pairs: Vec<String> = (0..100).map(|i| format!("key{i}")).collect();
    for key in &pairs {
        mset.push(key.as_bytes());
        mset.push(b"value");
    }
    input.extend(request(&mset));

    let mut expected = Vec::new();
    parse_requests(&mut BytesMut::from(&input[..]), &mut expected).unwrap();
    assert_eq!(expected.len(), 4);

    let mut parser = RequestParser::new();
    let mut buf = BytesMut::new();
    let mut out = Vec::new();
    for byte in &input {
        buf.extend_from_slice(std::slice::from_ref(byte));
        parser.parse(&mut buf, &mut out).unwrap();
    }
    assert_eq!(out, expected);
    assert!(buf.is_empty());
}

#[test]
fn request_parser_reports_protocol_errors_and_can_be_reset() {
    let mut parser = RequestParser::new();
    let mut buf = BytesMut::from(&b"*2\r\n$3\r\nGET\r\n"[..]);
    let mut out = Vec::new();
    parser.parse(&mut buf, &mut out).unwrap();
    assert!(out.is_empty(), "the request is not complete yet");
    buf.extend_from_slice(b"X");
    let err = parser.parse(&mut buf, &mut out).unwrap_err();
    assert_eq!(err.to_string(), "ERR Protocol error: expected '$', got 'X'");

    parser.reset();
    let mut buf = BytesMut::from(&request(&[b"PING"])[..]);
    parser.parse(&mut buf, &mut out).unwrap();
    assert_eq!(out, vec![Request::Cmd(Cmd::Ping(None))]);
}

#[test]
fn request_parser_reads_a_large_request_in_small_pieces_in_linear_time() {
    // 200,000 keys (about 2.6 MB) arriving in 4 KiB reads. Parsing the whole
    // request again on every read would copy about 2.6 MB x 650 / 2.
    let keys: Vec<String> = (0..200_000).map(|i| format!("k{i:06}")).collect();
    let mut args: Vec<&[u8]> = vec![b"DEL"];
    args.extend(keys.iter().map(|k| k.as_bytes()));
    let input = request(&args);

    let started = Instant::now();
    let mut parser = RequestParser::new();
    let mut buf = BytesMut::new();
    let mut out = Vec::new();
    for piece in input.chunks(4096) {
        buf.extend_from_slice(piece);
        parser.parse(&mut buf, &mut out).unwrap();
    }
    let elapsed = started.elapsed();
    match &out[..] {
        [Request::Cmd(Cmd::Del(parsed))] => assert_eq!(parsed.len(), keys.len()),
        other => panic!("unexpected result: {} requests", other.len()),
    }
    assert!(
        elapsed < Duration::from_secs(5),
        "parsing a fragmented 200k-argument request took {elapsed:?}"
    );
}
