//! Connection behaviour over TCP: error replies, pipelining, protocol errors
//! and half-closed clients.
//!
//! These tests talk to a running server on 127.0.0.1:7379 (for example the
//! one `.hub/sunucu-testleri.sh` starts) and are ignored by default.

mod common;

use common::req;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ADDR: &str = "127.0.0.1:7379";

fn connect() -> TcpStream {
    let stream = TcpStream::connect(ADDR).expect("connect to ignix on 127.0.0.1:7379");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.set_nodelay(true).unwrap();
    stream
}

/// A key no other test or earlier run uses (the server keeps its data).
fn unique_key(name: &str) -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!(
        "network-test:{name}:{}:{nanos}:{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Read one complete RESP2 or RESP3 reply and return its raw bytes.
fn read_reply(reader: &mut impl BufRead) -> Vec<u8> {
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).expect("read reply");
    assert!(
        line.ends_with(b"\r\n"),
        "connection closed or reply truncated: {:?}",
        String::from_utf8_lossy(&line)
    );
    let mut reply = line.clone();
    let number = || -> i64 {
        std::str::from_utf8(&line[1..line.len() - 2])
            .unwrap()
            .parse()
            .unwrap()
    };
    match line[0] {
        // Simple string, error, integer, RESP3 null, double, boolean, big number
        b'+' | b'-' | b':' | b'_' | b',' | b'#' | b'(' => {}
        // Bulk string, RESP3 verbatim string
        b'$' | b'=' => {
            let len = number();
            if len >= 0 {
                let mut body = vec![0; len as usize + 2];
                reader.read_exact(&mut body).expect("read bulk body");
                reply.extend_from_slice(&body);
            }
        }
        // Array, RESP3 set and push
        b'*' | b'~' | b'>' => {
            for _ in 0..number().max(0) {
                reply.extend(read_reply(reader));
            }
        }
        // RESP3 map: keys and values
        b'%' => {
            for _ in 0..2 * number().max(0) {
                reply.extend(read_reply(reader));
            }
        }
        other => panic!("unexpected reply type {:?}", other as char),
    }
    reply
}

/// Assert that the server closed the connection without sending more data.
fn assert_eof(reader: &mut impl Read) {
    let mut buf = [0u8; 256];
    match reader.read(&mut buf) {
        Ok(0) => {}
        Ok(n) => panic!(
            "expected the server to close the connection, got {:?}",
            String::from_utf8_lossy(&buf[..n])
        ),
        Err(e) if e.kind() == ErrorKind::ConnectionReset => {}
        Err(e) => panic!("expected the server to close the connection, got error: {e}"),
    }
}

/// Send `data` in one write, then read `count` replies.
fn roundtrip(data: &[u8], count: usize) -> Vec<Vec<u8>> {
    let mut stream = connect();
    stream.write_all(data).unwrap();
    let mut reader = BufReader::new(stream);
    (0..count).map(|_| read_reply(&mut reader)).collect()
}

#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn unknown_command_gets_an_error_and_the_connection_stays_usable() {
    let mut stream = connect();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    stream.write_all(&req(&[b"FOO", b"bar"])).unwrap();
    assert_eq!(
        read_reply(&mut reader),
        b"-ERR unknown command 'FOO', with args beginning with: 'bar' \r\n"
    );
    stream.write_all(&req(&[b"PING"])).unwrap();
    assert_eq!(read_reply(&mut reader), b"+PONG\r\n");
}

#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn client_setinfo_handshake_is_accepted() {
    // redis-py 5+ and node-redis 4.7 send these right after connecting.
    let mut data = req(&[b"CLIENT", b"SETINFO", b"LIB-NAME", b"redis-py"]);
    data.extend(req(&[b"CLIENT", b"SETINFO", b"LIB-VER", b"5.0.0"]));
    data.extend(req(&[b"PING"]));
    let replies = roundtrip(&data, 3);
    assert_eq!(replies, [&b"+OK\r\n"[..], b"+OK\r\n", b"+PONG\r\n"]);
}

#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn pipeline_with_an_invalid_command_answers_every_request_in_order() {
    let key = unique_key("pipeline");
    let mut data = req(&[b"SET", key.as_bytes(), b"1"]);
    data.extend(req(&[b"FOO"]));
    data.extend(req(&[b"GET", key.as_bytes()]));
    let replies = roundtrip(&data, 3);
    assert_eq!(replies[0], b"+OK\r\n");
    assert_eq!(
        replies[1],
        b"-ERR unknown command 'FOO', with args beginning with: \r\n"
    );
    assert_eq!(replies[2], b"$1\r\n1\r\n");
}

#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn unknown_command_error_replaces_line_breaks() {
    let replies = roundtrip(&req(&[b"A\rB\nC"]), 1);
    assert_eq!(
        replies[0],
        b"-ERR unknown command 'A B C', with args beginning with: \r\n"
    );
}

#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn malformed_frame_gets_a_protocol_error_and_the_connection_is_closed() {
    let mut stream = connect();
    stream.write_all(b"*1\r\nX").unwrap();
    let mut reader = BufReader::new(stream);
    assert_eq!(
        read_reply(&mut reader),
        b"-ERR Protocol error: expected '$', got 'X'\r\n"
    );
    assert_eof(&mut reader);
}

#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn requests_before_a_protocol_error_are_executed() {
    let key = unique_key("before-protocol-error");
    let mut stream = connect();
    let mut data = req(&[b"SET", key.as_bytes(), b"v"]);
    data.extend_from_slice(b"*1\r\nX");
    stream.write_all(&data).unwrap();
    let mut reader = BufReader::new(stream);
    assert_eq!(read_reply(&mut reader), b"+OK\r\n");
    assert_eq!(
        read_reply(&mut reader),
        b"-ERR Protocol error: expected '$', got 'X'\r\n"
    );
    assert_eof(&mut reader);

    let replies = roundtrip(&req(&[b"GET", key.as_bytes()]), 1);
    assert_eq!(replies[0], b"$1\r\nv\r\n");
}

#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn quit_closes_the_connection_and_drops_the_rest_of_the_pipeline() {
    let key = unique_key("after-quit");
    let mut stream = connect();
    let mut data = req(&[b"PING"]);
    data.extend(req(&[b"QUIT"]));
    data.extend(req(&[b"SET", key.as_bytes(), b"v"]));
    data.extend(req(&[b"PING"]));
    stream.write_all(&data).unwrap();
    let mut reader = BufReader::new(stream);
    assert_eq!(read_reply(&mut reader), b"+PONG\r\n");
    assert_eq!(read_reply(&mut reader), b"+OK\r\n");
    assert_eof(&mut reader);

    let replies = roundtrip(&req(&[b"GET", key.as_bytes()]), 1);
    assert_eq!(replies[0], b"$-1\r\n");
}

#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn hello_3_switches_the_connection_to_resp3() {
    let key = unique_key("resp3-missing");
    let mut data = req(&[b"HELLO", b"3"]);
    data.extend(req(&[b"GET", key.as_bytes()]));
    data.extend(req(&[b"HELLO", b"2"]));
    data.extend(req(&[b"GET", key.as_bytes()]));
    let replies = roundtrip(&data, 4);
    assert!(
        replies[0].starts_with(b"%7\r\n$6\r\nserver\r\n$5\r\nredis\r\n"),
        "{:?}",
        String::from_utf8_lossy(&replies[0])
    );
    assert!(replies[0].ends_with(b"$7\r\nmodules\r\n*0\r\n"));
    assert_eq!(replies[1], b"_\r\n");
    assert!(replies[2].starts_with(b"*14\r\n"));
    assert_eq!(replies[3], b"$-1\r\n");
}

/// The value of `field` in an INFO reply
fn info_field(reply: &[u8], field: &str) -> u64 {
    let text = String::from_utf8_lossy(reply);
    let prefix = format!("{field}:");
    text.lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("no {field} in {text:?}"))
        .parse()
        .unwrap()
}

#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn info_counts_connections_and_commands() {
    let mut stream = connect();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    stream.write_all(&req(&[b"INFO", b"stats"])).unwrap();
    let before = read_reply(&mut reader);
    // Another connection, and two more commands on this one
    roundtrip(&req(&[b"PING"]), 1);
    let mut data = req(&[b"PING"]);
    data.extend(req(&[b"INFO"]));
    stream.write_all(&data).unwrap();
    read_reply(&mut reader);
    let after = read_reply(&mut reader);
    let received = |reply: &[u8]| info_field(reply, "total_connections_received");
    let commands = |reply: &[u8]| info_field(reply, "total_commands_processed");
    assert!(received(&after) > received(&before));
    assert!(commands(&after) >= commands(&before) + 3);
    assert!(info_field(&after, "connected_clients") >= 1);
    assert_eq!(info_field(&after, "tcp_port"), 7379);

    let replies = roundtrip(&req(&[b"CONFIG", b"GET", b"port"]), 1);
    assert_eq!(replies[0], b"*2\r\n$4\r\nport\r\n$4\r\n7379\r\n");
}

#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn empty_multibulk_gets_no_reply() {
    let mut data = b"*0\r\n".to_vec();
    data.extend(req(&[b"PING"]));
    let replies = roundtrip(&data, 1);
    assert_eq!(replies[0], b"+PONG\r\n");
}

/// Send `data` and half-close in the same TCP segment: with TCP_CORK the
/// FIN is attached to the queued data, so the server sees both in one read.
#[cfg(target_os = "linux")]
fn send_and_half_close(stream: &mut TcpStream, data: &[u8]) {
    socket2::SockRef::from(&*stream).set_cork(true).unwrap();
    stream.write_all(data).unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn half_closed_client_still_gets_its_replies() {
    let key = unique_key("half-close");
    let mut stream = connect();
    let mut data = req(&[b"SET", key.as_bytes(), b"v"]);
    data.extend(req(&[b"GET", key.as_bytes()]));
    send_and_half_close(&mut stream, &data);
    let mut reader = BufReader::new(stream);
    assert_eq!(read_reply(&mut reader), b"+OK\r\n");
    assert_eq!(read_reply(&mut reader), b"$1\r\nv\r\n");
    assert_eof(&mut reader);
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn large_reply_is_flushed_after_the_client_half_closes() {
    let key = unique_key("large-half-close");
    let value = vec![b'L'; 16 * 1024 * 1024];
    let replies = roundtrip(&req(&[b"SET", key.as_bytes(), &value]), 1);
    assert_eq!(replies[0], b"+OK\r\n");

    let mut stream = connect();
    send_and_half_close(&mut stream, &req(&[b"GET", key.as_bytes()]));
    let mut reader = BufReader::new(stream);
    let reply = read_reply(&mut reader);
    let header = format!("${}\r\n", value.len());
    assert!(reply.starts_with(header.as_bytes()));
    assert_eq!(reply.len(), header.len() + value.len() + 2);
    assert!(reply[header.len()..header.len() + value.len()]
        .iter()
        .all(|&b| b == b'L'));
    assert_eof(&mut reader);
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn half_closed_client_with_a_pipeline_longer_than_the_read_buffer_gets_every_reply() {
    // Several times the server's 4 KiB read buffer, with the FIN in the same
    // burst: full reads, a short read and EOF all come from one event.
    let key = unique_key("half-close-pipeline");
    let count = 300;
    let data = req(&[b"SET", key.as_bytes(), b"value"]).repeat(count);
    let mut stream = connect();
    send_and_half_close(&mut stream, &data);
    let mut reader = BufReader::new(stream);
    for _ in 0..count {
        assert_eq!(read_reply(&mut reader), b"+OK\r\n");
    }
    assert_eof(&mut reader);
}

#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn half_close_after_the_replies_closes_the_connection() {
    let mut stream = connect();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    stream.write_all(&req(&[b"PING"])).unwrap();
    assert_eq!(read_reply(&mut reader), b"+PONG\r\n");
    // The FIN arrives on its own, after the request was read.
    stream.shutdown(Shutdown::Write).unwrap();
    assert_eof(&mut reader);
}

#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn request_sent_one_byte_at_a_time_is_answered() {
    let key = unique_key("byte-by-byte");
    let mut stream = connect();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    // With TCP_NODELAY every byte is its own segment and its own read.
    for byte in req(&[b"SET", key.as_bytes(), b"v"]) {
        stream.write_all(&[byte]).unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(read_reply(&mut reader), b"+OK\r\n");
    stream.write_all(&req(&[b"GET", key.as_bytes()])).unwrap();
    assert_eq!(read_reply(&mut reader), b"$1\r\nv\r\n");
}

#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn many_pipelined_pings_are_all_answered() {
    let count = 10_000;
    let data = req(&[b"PING"]).repeat(count);
    let replies = roundtrip(&data, count);
    assert!(replies.iter().all(|r| r == b"+PONG\r\n"));
}

#[test]
#[ignore = "requires a running ignix server on 127.0.0.1:7379"]
fn negative_bulk_length_gets_a_protocol_error_instead_of_a_crash() {
    let mut stream = connect();
    stream.write_all(b"*1\r\n$-5\r\n").unwrap();
    let mut reader = BufReader::new(stream);
    assert_eq!(
        read_reply(&mut reader),
        b"-ERR Protocol error: invalid bulk length\r\n"
    );
    assert_eof(&mut reader);

    let replies = roundtrip(&req(&[b"PING"]), 1);
    assert_eq!(replies[0], b"+PONG\r\n");
}
