//! Command semantics, checked against the replies of Redis 7.0.15.
//!
//! Requests are raw RESP and replies are compared byte for byte, so these
//! tests do not depend on the shape of the `Cmd` variants.

mod common;

use common::exec;
use ignix::Shard;

fn shard() -> Shard {
    Shard::new(0, None)
}

fn arity_error(name: &str) -> Vec<u8> {
    format!("-ERR wrong number of arguments for '{name}' command\r\n").into_bytes()
}

#[test]
fn del_removes_all_given_keys_and_counts_them() {
    let s = shard();
    exec(&s, &[b"SET", b"a", b"1"]);
    exec(&s, &[b"SET", b"b", b"2"]);
    assert_eq!(exec(&s, &[b"DEL", b"a", b"b", b"missing", b"a"]), b":2\r\n");
    assert_eq!(exec(&s, &[b"GET", b"a"]), b"$-1\r\n");
    assert_eq!(exec(&s, &[b"GET", b"b"]), b"$-1\r\n");
}

#[test]
fn exists_counts_every_existing_argument_including_duplicates() {
    let s = shard();
    exec(&s, &[b"SET", b"x", b"1"]);
    assert_eq!(exec(&s, &[b"EXISTS", b"x", b"x", b"missing"]), b":2\r\n");
}

#[test]
fn ping_without_message_replies_pong() {
    assert_eq!(exec(&shard(), &[b"PING"]), b"+PONG\r\n");
}

#[test]
fn ping_with_message_echoes_it_as_bulk_string() {
    assert_eq!(exec(&shard(), &[b"PING", b"hello"]), b"$5\r\nhello\r\n");
}

#[test]
fn get_with_extra_argument_is_an_arity_error() {
    let s = shard();
    exec(&s, &[b"SET", b"a", b"1"]);
    assert_eq!(exec(&s, &[b"GET", b"a", b"b"]), arity_error("get"));
}

#[test]
fn wrong_number_of_arguments_matches_redis_error() {
    let cases: &[(&[&[u8]], &str)] = &[
        (&[b"PING", b"a", b"b"], "ping"),
        (&[b"GET"], "get"),
        (&[b"SET", b"k"], "set"),
        (&[b"DEL"], "del"),
        (&[b"EXISTS"], "exists"),
        (&[b"INCR"], "incr"),
        (&[b"INCR", b"a", b"b"], "incr"),
        (&[b"RENAME", b"a"], "rename"),
        (&[b"RENAME", b"a", b"b", b"c"], "rename"),
        (&[b"MGET"], "mget"),
        (&[b"MSET", b"a"], "mset"),
        (&[b"MSET", b"a", b"1", b"b"], "mset"),
    ];
    for (args, name) in cases {
        assert_eq!(
            exec(&shard(), args),
            arity_error(name),
            "request {:?}",
            args.iter()
                .map(|a| String::from_utf8_lossy(a))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn unknown_command_error_matches_redis_format() {
    assert_eq!(
        exec(&shard(), &[b"FOO", b"bar", b"baz"]),
        b"-ERR unknown command 'FOO', with args beginning with: 'bar' 'baz' \r\n"
    );
    assert_eq!(
        exec(&shard(), &[b"FOO"]),
        b"-ERR unknown command 'FOO', with args beginning with: \r\n"
    );
}

#[test]
fn unknown_command_error_truncates_like_redis() {
    let long = vec![b'a'; 200];
    let expected = format!(
        "-ERR unknown command 'FOO', with args beginning with: '{}' \r\n",
        "a".repeat(128)
    );
    assert_eq!(exec(&shard(), &[b"FOO", &long, b"b"]), expected.as_bytes());

    let hundred = vec![b'x'; 100];
    let expected = format!(
        "-ERR unknown command 'FOO', with args beginning with: '{}' '{}' \r\n",
        "x".repeat(100),
        "x".repeat(25)
    );
    assert_eq!(
        exec(&shard(), &[b"FOO", &hundred, &hundred, b"z"]),
        expected.as_bytes()
    );

    let name = vec![b'N'; 200];
    let expected = format!(
        "-ERR unknown command '{}', with args beginning with: \r\n",
        "N".repeat(128)
    );
    assert_eq!(exec(&shard(), &[&name]), expected.as_bytes());
}

#[test]
fn set_with_expiry_option_is_rejected_and_key_not_written() {
    let s = shard();
    assert_eq!(
        exec(&s, &[b"SET", b"k", b"v", b"EX", b"10"]),
        b"-ERR SET option 'EX' is not supported\r\n"
    );
    assert_eq!(exec(&s, &[b"GET", b"k"]), b"$-1\r\n");
}

#[test]
fn set_options_are_matched_case_insensitively() {
    assert_eq!(
        exec(&shard(), &[b"SET", b"k", b"v", b"nx"]),
        b"-ERR SET option 'NX' is not supported\r\n"
    );
}

#[test]
fn set_with_unknown_token_is_a_syntax_error() {
    let s = shard();
    assert_eq!(
        exec(&s, &[b"SET", b"k", b"v", b"FOO"]),
        b"-ERR syntax error\r\n"
    );
    assert_eq!(exec(&s, &[b"GET", b"k"]), b"$-1\r\n");
}

#[test]
fn command_names_are_case_insensitive() {
    let s = shard();
    assert_eq!(exec(&s, &[b"set", b"k", b"v"]), b"+OK\r\n");
    assert_eq!(exec(&s, &[b"GeT", b"k"]), b"$1\r\nv\r\n");
}

fn bulk(value: &[u8]) -> Vec<u8> {
    [
        format!("${}\r\n", value.len()).as_bytes(),
        value,
        b"\r\n".as_slice(),
    ]
    .concat()
}

#[test]
fn set_preserves_non_canonical_integer_strings() {
    let s = shard();
    let values: [&[u8]; 10] = [
        b"007",
        b"-0",
        b"00",
        b"+1",
        b" 1",
        b"1 ",
        b"0x10",
        b"1e3",
        b"9223372036854775808",
        b"-9223372036854775809",
    ];
    for value in values {
        exec(&s, &[b"SET", b"k", value]);
        assert_eq!(
            exec(&s, &[b"GET", b"k"]),
            bulk(value),
            "value {:?}",
            String::from_utf8_lossy(value)
        );
    }
}

#[test]
fn mset_preserves_non_canonical_integer_strings() {
    let s = shard();
    exec(&s, &[b"MSET", b"a", b"007", b"b", b"-0"]);
    assert_eq!(
        exec(&s, &[b"MGET", b"a", b"b"]),
        b"*2\r\n$3\r\n007\r\n$2\r\n-0\r\n"
    );
}

#[test]
fn set_round_trips_canonical_integers() {
    let s = shard();
    let values: [&[u8]; 5] = [
        b"0",
        b"-1",
        b"42",
        b"9223372036854775807",
        b"-9223372036854775808",
    ];
    for value in values {
        exec(&s, &[b"SET", b"k", value]);
        assert_eq!(exec(&s, &[b"GET", b"k"]), bulk(value));
    }
}
