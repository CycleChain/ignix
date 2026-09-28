//! Command semantics, checked against the replies of Redis 7.0.15.
//!
//! Requests are raw RESP and replies are compared byte for byte, so these
//! tests do not depend on the shape of the `Cmd` variants.

mod common;

use common::{exec, exec_in};
use ignix::{Protocol, Session, Shard};

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
fn echo_replies_with_its_argument() {
    let s = shard();
    assert_eq!(exec(&s, &[b"ECHO", b"hello"]), b"$5\r\nhello\r\n");
    assert_eq!(exec(&s, &[b"ECHO", b""]), b"$0\r\n\r\n");
    assert_eq!(exec(&s, &[b"ECHO"]), arity_error("echo"));
    assert_eq!(exec(&s, &[b"ECHO", b"a", b"b"]), arity_error("echo"));
}

#[test]
fn select_accepts_only_database_zero_with_redis_errors() {
    let s = shard();
    assert_eq!(exec(&s, &[b"SELECT", b"0"]), b"+OK\r\n");
    let out_of_range = b"-ERR DB index is out of range\r\n";
    assert_eq!(exec(&s, &[b"SELECT", b"1"]), out_of_range);
    assert_eq!(exec(&s, &[b"SELECT", b"-1"]), out_of_range);
    let not_an_integer = b"-ERR value is not an integer or out of range\r\n";
    assert_eq!(exec(&s, &[b"SELECT", b"abc"]), not_an_integer);
    assert_eq!(exec(&s, &[b"SELECT", b"01"]), not_an_integer);
    assert_eq!(
        exec(&s, &[b"SELECT", b"2147483648"]),
        b"-ERR value is out of range, value must between -2147483648 and 2147483647\r\n"
    );
    assert_eq!(exec(&s, &[b"SELECT"]), arity_error("select"));
}

#[test]
fn quit_replies_ok_and_takes_any_arguments() {
    let s = shard();
    assert_eq!(exec(&s, &[b"QUIT"]), b"+OK\r\n");
    assert_eq!(exec(&s, &[b"QUIT", b"now"]), b"+OK\r\n");
}

/// The HELLO reply Redis 7 sends, with Ignix's version and client `id`
fn hello_reply(protocol: u8, id: u64) -> Vec<u8> {
    let header = if protocol == 3 { "%7" } else { "*14" };
    format!(
        "{header}\r\n$6\r\nserver\r\n$5\r\nredis\r\n$7\r\nversion\r\n$5\r\n7.0.0\r\n\
         $5\r\nproto\r\n:{protocol}\r\n$2\r\nid\r\n:{id}\r\n$4\r\nmode\r\n$10\r\nstandalone\r\n\
         $4\r\nrole\r\n$6\r\nmaster\r\n$7\r\nmodules\r\n*0\r\n"
    )
    .into_bytes()
}

#[test]
fn hello_describes_the_server_in_the_chosen_protocol() {
    let s = shard();
    let mut session = Session::new();
    let id = session.id();
    assert_eq!(exec_in(&s, &mut session, &[b"HELLO"]), hello_reply(2, id));
    assert_eq!(
        exec_in(&s, &mut session, &[b"HELLO", b"3"]),
        hello_reply(3, id)
    );
    assert_eq!(session.protocol(), Protocol::Resp3);
    // Without a version HELLO keeps the protocol
    assert_eq!(exec_in(&s, &mut session, &[b"HELLO"]), hello_reply(3, id));
    assert_eq!(
        exec_in(&s, &mut session, &[b"HELLO", b"2"]),
        hello_reply(2, id)
    );
    assert_eq!(session.protocol(), Protocol::Resp2);
}

#[test]
fn resp3_sends_null_as_an_underscore() {
    let s = shard();
    let mut session = Session::new();
    exec_in(&s, &mut session, &[b"SET", b"k", b"v"]);
    exec_in(&s, &mut session, &[b"HELLO", b"3"]);
    assert_eq!(exec_in(&s, &mut session, &[b"GET", b"missing"]), b"_\r\n");
    assert_eq!(
        exec_in(&s, &mut session, &[b"MGET", b"k", b"missing"]),
        b"*2\r\n$1\r\nv\r\n_\r\n"
    );
    exec_in(&s, &mut session, &[b"HELLO", b"2"]);
    assert_eq!(exec_in(&s, &mut session, &[b"GET", b"missing"]), b"$-1\r\n");
}

#[test]
fn hello_rejects_unknown_versions_and_options_like_redis() {
    let s = shard();
    let noproto = b"-NOPROTO unsupported protocol version\r\n";
    assert_eq!(exec(&s, &[b"HELLO", b"4"]), noproto);
    assert_eq!(exec(&s, &[b"HELLO", b"1"]), noproto);
    assert_eq!(exec(&s, &[b"HELLO", b"-1"]), noproto);
    let not_an_integer = b"-ERR Protocol version is not an integer or out of range\r\n";
    assert_eq!(exec(&s, &[b"HELLO", b"abc"]), not_an_integer);
    assert_eq!(exec(&s, &[b"HELLO", b"03"]), not_an_integer);
    assert_eq!(exec(&s, &[b"HELLO", b"SETNAME", b"x"]), not_an_integer);
    assert_eq!(
        exec(&s, &[b"HELLO", b"3", b"FOO"]),
        b"-ERR Syntax error in HELLO option 'FOO'\r\n"
    );
    // AUTH needs two arguments and SETNAME one
    assert_eq!(
        exec(&s, &[b"HELLO", b"3", b"AUTH", b"default"]),
        b"-ERR Syntax error in HELLO option 'AUTH'\r\n"
    );
    assert_eq!(
        exec(&s, &[b"HELLO", b"3", b"SETNAME"]),
        b"-ERR Syntax error in HELLO option 'SETNAME'\r\n"
    );
    assert_eq!(
        exec(&s, &[b"HELLO", b"3", b"SETNAME", b"a b"]),
        b"-ERR Client names cannot contain spaces, newlines or special characters.\r\n"
    );
}

#[test]
fn hello_auth_accepts_only_the_default_user_without_a_password() {
    let s = shard();
    let mut session = Session::new();
    let id = session.id();
    assert_eq!(
        exec_in(
            &s,
            &mut session,
            &[b"HELLO", b"3", b"AUTH", b"default", b"any"]
        ),
        hello_reply(3, id)
    );
    let wrongpass = b"-WRONGPASS invalid username-password pair or user is disabled.\r\n";
    assert_eq!(
        exec(&s, &[b"HELLO", b"3", b"AUTH", b"DEFAULT", b"x"]),
        wrongpass
    );
    assert_eq!(
        exec(&s, &[b"HELLO", b"3", b"AUTH", b"other", b"x"]),
        wrongpass
    );
}

#[test]
fn hello_applies_options_in_order_until_one_fails() {
    let s = shard();
    let mut session = Session::new();
    let id = session.id();
    // The last SETNAME wins
    let reply = exec_in(
        &s,
        &mut session,
        &[b"HELLO", b"3", b"setname", b"n1", b"SETNAME", b"n2"],
    );
    assert_eq!(reply, hello_reply(3, id));
    assert_eq!(session.name().map(|n| &n[..]), Some(&b"n2"[..]));
    // A failing option stops HELLO, but the options before it stay applied
    // and the protocol does not change
    let mut session = Session::new();
    let reply = exec_in(
        &s,
        &mut session,
        &[b"HELLO", b"3", b"SETNAME", b"good", b"FOO"],
    );
    assert_eq!(reply, b"-ERR Syntax error in HELLO option 'FOO'\r\n");
    assert_eq!(session.name().map(|n| &n[..]), Some(&b"good"[..]));
    assert_eq!(session.protocol(), Protocol::Resp2);
    // AUTH is checked where it appears, before a later SETNAME
    let reply = exec_in(
        &s,
        &mut session,
        &[
            b"HELLO",
            b"3",
            b"AUTH",
            b"other",
            b"pw",
            b"SETNAME",
            b"bad name",
        ],
    );
    assert_eq!(
        reply,
        b"-WRONGPASS invalid username-password pair or user is disabled.\r\n"
    );
    // An empty name removes the name
    exec_in(&s, &mut session, &[b"HELLO", b"2", b"SETNAME", b""]);
    assert_eq!(session.name(), None);
}

#[test]
fn client_id_is_the_session_id() {
    let s = shard();
    let mut session = Session::new();
    let id = session.id();
    assert_eq!(
        exec_in(&s, &mut session, &[b"CLIENT", b"ID"]),
        format!(":{id}\r\n").into_bytes()
    );
    assert_eq!(
        exec_in(&s, &mut session, &[b"client", b"id"]),
        format!(":{id}\r\n").into_bytes()
    );
}

#[test]
fn client_setname_and_getname() {
    let s = shard();
    let mut session = Session::new();
    assert_eq!(
        exec_in(&s, &mut session, &[b"CLIENT", b"GETNAME"]),
        b"$-1\r\n"
    );
    assert_eq!(
        exec_in(&s, &mut session, &[b"CLIENT", b"SETNAME", b"n1"]),
        b"+OK\r\n"
    );
    assert_eq!(
        exec_in(&s, &mut session, &[b"CLIENT", b"GETNAME"]),
        b"$2\r\nn1\r\n"
    );
    assert_eq!(
        exec_in(&s, &mut session, &[b"CLIENT", b"SETNAME", b"a b"]),
        b"-ERR Client names cannot contain spaces, newlines or special characters.\r\n"
    );
    // An empty name removes the name; RESP3 sends the null as "_"
    exec_in(&s, &mut session, &[b"CLIENT", b"SETNAME", b""]);
    exec_in(&s, &mut session, &[b"HELLO", b"3"]);
    assert_eq!(
        exec_in(&s, &mut session, &[b"CLIENT", b"GETNAME"]),
        b"_\r\n"
    );
}

#[test]
fn client_setinfo_records_the_library_name_and_version() {
    let s = shard();
    let mut session = Session::new();
    for (attribute, value) in [(&b"LIB-NAME"[..], &b"redis-py"[..]), (b"lib-ver", b"8.1.0")] {
        assert_eq!(
            exec_in(&s, &mut session, &[b"CLIENT", b"SETINFO", attribute, value]),
            b"+OK\r\n"
        );
    }
    assert_eq!(session.lib_name().map(|v| &v[..]), Some(&b"redis-py"[..]));
    assert_eq!(session.lib_ver().map(|v| &v[..]), Some(&b"8.1.0"[..]));
    assert_eq!(
        exec(&s, &[b"CLIENT", b"SETINFO", b"LIB-FOO", b"x"]),
        b"-ERR Unrecognized option 'LIB-FOO'\r\n"
    );
    assert_eq!(
        exec(&s, &[b"CLIENT", b"SETINFO", b"LIB-NAME", b"a b"]),
        b"-ERR LIB-NAME cannot contain spaces, newlines or special characters.\r\n"
    );
}

#[test]
fn client_checks_subcommands_and_their_arity_like_redis() {
    let s = shard();
    assert_eq!(exec(&s, &[b"CLIENT"]), arity_error("client"));
    assert_eq!(
        exec(&s, &[b"CLIENT", b"FOO"]),
        b"-ERR unknown subcommand 'FOO'. Try CLIENT HELP.\r\n"
    );
    assert_eq!(
        exec(&s, &[b"CLIENT", b"foo", b"bar"]),
        b"-ERR unknown subcommand 'foo'. Try CLIENT HELP.\r\n"
    );
    for (args, name) in [
        (&[&b"CLIENT"[..], b"ID", b"x"][..], "client|id"),
        (&[b"CLIENT", b"GETNAME", b"x"], "client|getname"),
        (&[b"CLIENT", b"SETNAME"], "client|setname"),
        (&[b"CLIENT", b"SETNAME", b"a", b"b"], "client|setname"),
        (&[b"CLIENT", b"SETINFO", b"LIB-NAME"], "client|setinfo"),
    ] {
        assert_eq!(exec(&s, args), arity_error(name));
    }
}

#[test]
fn client_help_lists_the_supported_subcommands() {
    let s = shard();
    let reply = exec(&s, &[b"CLIENT", b"HELP"]);
    assert!(reply.starts_with(
        b"*13\r\n+CLIENT <subcommand> [<arg> [value] [opt] ...]. Subcommands are:\r\n"
    ));
    assert!(reply.ends_with(b"+HELP\r\n+    Prints this help.\r\n"));
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
fn mget_keeps_the_order_of_small_large_and_missing_values() {
    let s = shard();
    // Values over 16 KiB are written after the shard locks are released
    let large = vec![b'x'; 20_000];
    exec(
        &s,
        &[b"MSET", b"small", b"v", b"large", &large, b"int", b"42"],
    );
    let reply = exec(
        &s,
        &[b"MGET", b"small", b"large", b"missing", b"int", b"small"],
    );
    let expected = [
        b"*5\r\n".as_slice(),
        &bulk(b"v"),
        &bulk(&large),
        b"$-1\r\n",
        &bulk(b"42"),
        &bulk(b"v"),
    ]
    .concat();
    assert_eq!(reply, expected);
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

#[test]
fn incr_on_non_integer_returns_error_and_keeps_value() {
    let s = shard();
    exec(&s, &[b"SET", b"k", b"abc"]);
    assert_eq!(
        exec(&s, &[b"INCR", b"k"]),
        b"-ERR value is not an integer or out of range\r\n"
    );
    assert_eq!(exec(&s, &[b"GET", b"k"]), b"$3\r\nabc\r\n");
}

#[test]
fn incr_on_non_canonical_integer_returns_error() {
    let s = shard();
    exec(&s, &[b"SET", b"k", b"007"]);
    assert_eq!(
        exec(&s, &[b"INCR", b"k"]),
        b"-ERR value is not an integer or out of range\r\n"
    );
    assert_eq!(exec(&s, &[b"GET", b"k"]), b"$3\r\n007\r\n");
}

#[test]
fn incr_overflow_returns_error_and_keeps_value() {
    let s = shard();
    exec(&s, &[b"SET", b"k", b"9223372036854775807"]);
    assert_eq!(
        exec(&s, &[b"INCR", b"k"]),
        b"-ERR increment or decrement would overflow\r\n"
    );
    assert_eq!(exec(&s, &[b"GET", b"k"]), b"$19\r\n9223372036854775807\r\n");
}

#[test]
fn incr_missing_key_starts_at_one() {
    let s = shard();
    assert_eq!(exec(&s, &[b"INCR", b"counter"]), b":1\r\n");
    assert_eq!(exec(&s, &[b"INCR", b"counter"]), b":2\r\n");
    assert_eq!(exec(&s, &[b"GET", b"counter"]), b"$1\r\n2\r\n");
}

#[test]
fn incr_updates_integer_values() {
    let s = shard();
    exec(&s, &[b"SET", b"k", b"-10"]);
    assert_eq!(exec(&s, &[b"INCR", b"k"]), b":-9\r\n");
    assert_eq!(exec(&s, &[b"GET", b"k"]), b"$2\r\n-9\r\n");
}

#[test]
fn rename_missing_key_returns_resp_error() {
    assert_eq!(
        exec(&shard(), &[b"RENAME", b"nokey", b"other"]),
        b"-ERR no such key\r\n"
    );
}

#[test]
fn rename_missing_key_onto_itself_is_an_error() {
    assert_eq!(
        exec(&shard(), &[b"RENAME", b"nokey", b"nokey"]),
        b"-ERR no such key\r\n"
    );
}

#[test]
fn rename_existing_key_onto_itself_is_ok() {
    let s = shard();
    exec(&s, &[b"SET", b"k", b"v"]);
    assert_eq!(exec(&s, &[b"RENAME", b"k", b"k"]), b"+OK\r\n");
    assert_eq!(exec(&s, &[b"GET", b"k"]), b"$1\r\nv\r\n");
}

#[test]
fn rename_moves_the_value_and_overwrites_the_target() {
    let s = shard();
    exec(&s, &[b"SET", b"a", b"1"]);
    exec(&s, &[b"SET", b"b", b"2"]);
    assert_eq!(exec(&s, &[b"RENAME", b"a", b"b"]), b"+OK\r\n");
    assert_eq!(exec(&s, &[b"GET", b"a"]), b"$-1\r\n");
    assert_eq!(exec(&s, &[b"GET", b"b"]), b"$1\r\n1\r\n");
}

#[test]
fn incrby_decrby_and_decr_apply_the_delta() {
    let s = shard();
    exec(&s, &[b"SET", b"n", b"10"]);
    assert_eq!(exec(&s, &[b"INCRBY", b"n", b"5"]), b":15\r\n");
    assert_eq!(exec(&s, &[b"INCRBY", b"n", b"-20"]), b":-5\r\n");
    assert_eq!(exec(&s, &[b"DECRBY", b"n", b"3"]), b":-8\r\n");
    assert_eq!(exec(&s, &[b"DECR", b"n"]), b":-9\r\n");
    assert_eq!(exec(&s, &[b"INCRBY", b"new", b"7"]), b":7\r\n");
    assert_eq!(exec(&s, &[b"DECR", b"new2"]), b":-1\r\n");
    assert_eq!(exec(&s, &[b"GET", b"n"]), b"$2\r\n-9\r\n");
}

#[test]
fn incrby_rejects_a_non_canonical_increment() {
    let s = shard();
    exec(&s, &[b"SET", b"n", b"10"]);
    let increments: [&[u8]; 4] = [b"abc", b"+5", b"007", b" 5"];
    for increment in increments {
        assert_eq!(
            exec(&s, &[b"INCRBY", b"n", increment]),
            b"-ERR value is not an integer or out of range\r\n"
        );
    }
    assert_eq!(exec(&s, &[b"GET", b"n"]), b"$2\r\n10\r\n");
}

#[test]
fn incrby_and_decr_report_overflow_like_redis() {
    let s = shard();
    exec(&s, &[b"SET", b"max", b"9223372036854775800"]);
    assert_eq!(
        exec(&s, &[b"INCRBY", b"max", b"100"]),
        b"-ERR increment or decrement would overflow\r\n"
    );
    exec(&s, &[b"SET", b"min", b"-9223372036854775808"]);
    assert_eq!(
        exec(&s, &[b"DECR", b"min"]),
        b"-ERR increment or decrement would overflow\r\n"
    );
    assert_eq!(
        exec(&s, &[b"DECRBY", b"n", b"-9223372036854775808"]),
        b"-ERR decrement would overflow\r\n"
    );
}

#[test]
fn incrby_family_checks_arity() {
    let cases: &[(&[&[u8]], &str)] = &[
        (&[b"INCRBY", b"n"], "incrby"),
        (&[b"DECR"], "decr"),
        (&[b"DECRBY", b"n", b"1", b"2"], "decrby"),
    ];
    for (args, name) in cases {
        assert_eq!(exec(&shard(), args), arity_error(name));
    }
}
