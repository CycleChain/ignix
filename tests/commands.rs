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
fn dbsize_counts_the_keys() {
    let s = shard();
    assert_eq!(exec(&s, &[b"DBSIZE"]), b":0\r\n");
    exec(&s, &[b"MSET", b"a", b"1", b"b", b"2"]);
    assert_eq!(exec(&s, &[b"DBSIZE"]), b":2\r\n");
    assert_eq!(exec(&s, &[b"DBSIZE", b"x"]), arity_error("dbsize"));
}

#[test]
fn type_is_string_for_every_value_and_none_for_missing_keys() {
    let s = shard();
    exec(&s, &[b"SET", b"s", b"text"]);
    exec(&s, &[b"SET", b"i", b"42"]);
    assert_eq!(exec(&s, &[b"TYPE", b"s"]), b"+string\r\n");
    assert_eq!(exec(&s, &[b"TYPE", b"i"]), b"+string\r\n");
    assert_eq!(exec(&s, &[b"TYPE", b"missing"]), b"+none\r\n");
    assert_eq!(exec(&s, &[b"TYPE"]), arity_error("type"));
}

#[test]
fn unlink_deletes_like_del() {
    let s = shard();
    exec(&s, &[b"SET", b"a", b"1"]);
    assert_eq!(exec(&s, &[b"UNLINK", b"a", b"missing", b"a"]), b":1\r\n");
    assert_eq!(exec(&s, &[b"GET", b"a"]), b"$-1\r\n");
    assert_eq!(exec(&s, &[b"UNLINK"]), arity_error("unlink"));
}

#[test]
fn flushdb_and_flushall_remove_every_key() {
    let s = shard();
    for (flush, option) in [
        (&b"FLUSHDB"[..], None),
        (b"FLUSHDB", Some(&b"ASYNC"[..])),
        (b"FLUSHALL", Some(b"sync")),
        (b"FLUSHALL", None),
    ] {
        exec(&s, &[b"MSET", b"a", b"1", b"b", b"2"]);
        let mut args = vec![flush];
        args.extend(option);
        assert_eq!(exec(&s, &args), b"+OK\r\n");
        assert_eq!(exec(&s, &[b"DBSIZE"]), b":0\r\n");
    }
    // The keyspace works as before afterwards
    exec(&s, &[b"SET", b"k", b"v"]);
    assert_eq!(exec(&s, &[b"GET", b"k"]), b"$1\r\nv\r\n");
}

#[test]
fn flush_options_other_than_async_or_sync_are_syntax_errors() {
    let s = shard();
    for args in [
        &[&b"FLUSHDB"[..], b"foo"][..],
        &[b"FLUSHDB", b"async", b"sync"],
        &[b"FLUSHALL", b"x"],
    ] {
        assert_eq!(exec(&s, args), b"-ERR syntax error\r\n");
    }
}

/// Split a reply made of bulk strings and arrays of them into its strings
fn bulk_strings(reply: &[u8]) -> Vec<Vec<u8>> {
    let mut strings = Vec::new();
    let mut rest = reply;
    while let Some(end) = rest.windows(2).position(|w| w == b"\r\n") {
        let (line, after) = (&rest[..end], &rest[end + 2..]);
        rest = after;
        if line[0] == b'$' {
            let len: usize = std::str::from_utf8(&line[1..]).unwrap().parse().unwrap();
            strings.push(rest[..len].to_vec());
            rest = &rest[len + 2..];
        }
    }
    strings
}

/// The cursor and keys of a SCAN reply
fn scan_reply(reply: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut strings = bulk_strings(reply);
    assert!(
        reply.starts_with(b"*2\r\n"),
        "{:?}",
        String::from_utf8_lossy(reply)
    );
    let cursor = strings.remove(0);
    (cursor, strings)
}

/// Every key a full SCAN returns, running `between` after each step
fn full_scan(s: &Shard, options: &[&[u8]], mut between: impl FnMut()) -> Vec<Vec<u8>> {
    let mut cursor = b"0".to_vec();
    let mut keys = Vec::new();
    loop {
        let mut args: Vec<&[u8]> = vec![b"SCAN", &cursor];
        args.extend_from_slice(options);
        let (next, batch) = scan_reply(&exec(s, &args));
        keys.extend(batch);
        between();
        if next == b"0" {
            return keys;
        }
        cursor = next;
    }
}

#[test]
fn keys_returns_the_keys_matching_a_glob_pattern() {
    let s = shard();
    for key in [
        &b"hello"[..],
        b"hallo",
        b"hxllo",
        b"heeeello",
        b"other",
        b"",
    ] {
        exec(&s, &[b"SET", key, b"1"]);
    }
    let mut matched = bulk_strings(&exec(&s, &[b"KEYS", b"h?llo"]));
    matched.sort();
    assert_eq!(matched, [&b"hallo"[..], b"hello", b"hxllo"]);
    // `*` also returns the empty key, other patterns never match it
    assert_eq!(bulk_strings(&exec(&s, &[b"KEYS", b"*"])).len(), 6);
    assert_eq!(bulk_strings(&exec(&s, &[b"KEYS", b"**"])).len(), 5);
    assert_eq!(exec(&s, &[b"KEYS", b"nothing*"]), b"*0\r\n");
    assert_eq!(exec(&s, &[b"KEYS"]), arity_error("keys"));
}

#[test]
fn scan_returns_every_key_exactly_once() {
    let s = shard();
    for i in 0..10_000 {
        exec(&s, &[b"SET", format!("key:{i}").as_bytes(), b"v"]);
    }
    let mut keys = full_scan(&s, &[], || ());
    keys.sort();
    let mut expected: Vec<Vec<u8>> = (0..10_000)
        .map(|i| format!("key:{i}").into_bytes())
        .collect();
    expected.sort();
    assert_eq!(keys, expected);
}

#[test]
fn scan_returns_keys_that_exist_throughout_exactly_once_despite_writes() {
    let s = shard();
    for i in 0..10_000 {
        exec(&s, &[b"SET", format!("stable:{i}").as_bytes(), b"v"]);
        exec(&s, &[b"SET", format!("doomed:{i}").as_bytes(), b"v"]);
    }
    let mut step = 0;
    let keys = full_scan(&s, &[b"COUNT", b"100"], || {
        // Between steps, delete some keys and add new ones
        for i in step * 100..(step + 1) * 100 {
            exec(&s, &[b"DEL", format!("doomed:{i}").as_bytes()]);
            exec(&s, &[b"SET", format!("new:{step}:{i}").as_bytes(), b"v"]);
        }
        step += 1;
    });
    let mut stable: Vec<&Vec<u8>> = keys.iter().filter(|k| k.starts_with(b"stable:")).collect();
    stable.sort();
    stable.dedup();
    assert_eq!(stable.len(), 10_000);
    assert_eq!(
        keys.iter().filter(|k| k.starts_with(b"stable:")).count(),
        10_000
    );
    // Nothing is returned twice
    let mut all = keys.clone();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), keys.len());
}

#[test]
fn scan_filters_with_match_and_type_and_takes_count_as_a_hint() {
    let s = shard();
    for i in 0..100 {
        exec(&s, &[b"SET", format!("user:{i}").as_bytes(), b"v"]);
        exec(&s, &[b"SET", format!("item:{i}").as_bytes(), b"v"]);
    }
    let users = full_scan(&s, &[b"MATCH", b"user:*"], || ());
    assert_eq!(users.len(), 100);
    assert!(users.iter().all(|k| k.starts_with(b"user:")));
    // The last MATCH wins; TYPE is compared ignoring case
    let items = full_scan(&s, &[b"MATCH", b"user:*", b"match", b"item:*"], || ());
    assert_eq!(items.len(), 100);
    assert_eq!(full_scan(&s, &[b"TYPE", b"STRING"], || ()).len(), 200);
    assert!(full_scan(&s, &[b"TYPE", b"hash"], || ()).is_empty());
    // A large COUNT scans everything in one step
    let (cursor, keys) = scan_reply(&exec(&s, &[b"SCAN", b"0", b"COUNT", b"100000"]));
    assert_eq!((cursor, keys.len()), (b"0".to_vec(), 200));
}

#[test]
fn scan_parses_cursors_like_strtoul() {
    let s = shard();
    exec(&s, &[b"SET", b"k", b"v"]);
    for cursor in [&b""[..], b"0", b"+0", b"00"] {
        assert_eq!(
            scan_reply(&exec(&s, &[b"SCAN", cursor, b"COUNT", b"100000"]))
                .1
                .len(),
            1
        );
    }
    // -1 wraps around to a cursor past the end
    assert_eq!(exec(&s, &[b"SCAN", b"-1"]), b"*2\r\n$1\r\n0\r\n*0\r\n");
    for cursor in [
        &b" 0"[..],
        b"abc",
        b"0x1",
        b"+",
        b"-",
        b"18446744073709551616",
    ] {
        assert_eq!(exec(&s, &[b"SCAN", cursor]), b"-ERR invalid cursor\r\n");
    }
}

#[test]
fn scan_option_errors_match_redis() {
    let s = shard();
    let syntax = b"-ERR syntax error\r\n";
    assert_eq!(exec(&s, &[b"SCAN", b"0", b"COUNT", b"0"]), syntax);
    assert_eq!(exec(&s, &[b"SCAN", b"0", b"COUNT"]), syntax);
    assert_eq!(exec(&s, &[b"SCAN", b"0", b"FOO", b"x"]), syntax);
    assert_eq!(
        exec(&s, &[b"SCAN", b"0", b"COUNT", b"abc"]),
        b"-ERR value is not an integer or out of range\r\n"
    );
    // The cursor is checked first
    assert_eq!(
        exec(&s, &[b"SCAN", b"x", b"FOO"]),
        b"-ERR invalid cursor\r\n"
    );
    assert_eq!(exec(&s, &[b"SCAN"]), arity_error("scan"));
}

/// The text of an INFO reply (a bulk string)
fn info_text(s: &Shard, args: &[&[u8]]) -> String {
    let reply = exec(s, args);
    String::from_utf8(bulk_strings(&reply).remove(0)).unwrap()
}

#[test]
fn info_describes_the_server_in_redis_sections() {
    let s = shard();
    let text = info_text(&s, &[b"INFO"]);
    assert!(text.starts_with("# Server\r\nredis_version:7.0.0\r\nignix_version:"));
    let headers: Vec<&str> = text.lines().filter(|l| l.starts_with('#')).collect();
    assert_eq!(
        headers,
        [
            "# Server",
            "# Clients",
            "# Persistence",
            "# Stats",
            "# Replication",
            "# Keyspace"
        ]
    );
    // Sections are separated by an empty line, and the text ends with a line end
    assert!(text.contains("\r\n\r\n# Clients\r\nconnected_clients:"));
    assert!(text.ends_with("# Keyspace\r\n"));
    assert!(text.contains("\r\naof_enabled:0\r\n"));
    assert!(text.contains("\r\nrole:master\r\n"));
}

#[test]
fn info_keyspace_counts_the_keys() {
    let s = shard();
    assert_eq!(
        exec(&s, &[b"INFO", b"keyspace"]),
        b"$12\r\n# Keyspace\r\n\r\n"
    );
    exec(&s, &[b"MSET", b"a", b"1", b"b", b"2"]);
    assert_eq!(
        info_text(&s, &[b"INFO", b"KEYSPACE"]),
        "# Keyspace\r\ndb0:keys=2,expires=0,avg_ttl=0\r\n"
    );
}

#[test]
fn info_sections_come_in_redis_order_and_unknown_ones_are_left_out() {
    let s = shard();
    let text = info_text(&s, &[b"INFO", b"replication", b"clients", b"nosuch"]);
    assert!(text.starts_with("# Clients\r\n"), "{text}");
    assert!(text.contains("\r\n\r\n# Replication\r\n"), "{text}");
    assert_eq!(exec(&s, &[b"INFO", b"foo"]), b"$0\r\n\r\n");
    let everything = info_text(&s, &[b"INFO", b"everything"]);
    assert_eq!(everything.matches("\r\n# ").count() + 1, 6);
}

#[test]
fn info_is_a_verbatim_string_in_resp3() {
    let s = shard();
    let mut session = Session::new();
    exec_in(&s, &mut session, &[b"HELLO", b"3"]);
    assert_eq!(
        exec_in(&s, &mut session, &[b"INFO", b"keyspace"]),
        b"=16\r\ntxt:# Keyspace\r\n\r\n"
    );
}

#[test]
fn config_get_reports_parameters_by_name_or_pattern() {
    let s = shard();
    assert_eq!(
        exec(&s, &[b"CONFIG", b"GET", b"save"]),
        b"*2\r\n$4\r\nsave\r\n$0\r\n\r\n"
    );
    assert_eq!(
        exec(&s, &[b"CONFIG", b"GET", b"appendonly", b"save"]),
        b"*4\r\n$10\r\nappendonly\r\n$2\r\nno\r\n$4\r\nsave\r\n$0\r\n\r\n"
    );
    // An exact name is found ignoring case and reported as given
    assert_eq!(
        exec(&s, &[b"CONFIG", b"GET", b"MAXMEMORY"]),
        b"*2\r\n$9\r\nMAXMEMORY\r\n$1\r\n0\r\n"
    );
    // Patterns ignore case too; each parameter appears once
    let reply = exec(
        &s,
        &[
            b"CONFIG",
            b"GET",
            b"MAXMEMORY*",
            b"maxmemory",
            b"maxmemory-policy",
        ],
    );
    assert_eq!(
        bulk_strings(&reply),
        [&b"maxmemory"[..], b"0", b"maxmemory-policy", b"noeviction"]
    );
    assert_eq!(exec(&s, &[b"CONFIG", b"GET", b"nosuch"]), b"*0\r\n");
}

#[test]
fn config_get_is_a_map_in_resp3() {
    let s = shard();
    let mut session = Session::new();
    exec_in(&s, &mut session, &[b"HELLO", b"3"]);
    assert_eq!(
        exec_in(&s, &mut session, &[b"CONFIG", b"GET", b"databases"]),
        b"%1\r\n$9\r\ndatabases\r\n$1\r\n1\r\n"
    );
}

#[test]
fn config_checks_subcommands_and_their_arity_like_redis() {
    let s = shard();
    assert_eq!(exec(&s, &[b"CONFIG"]), arity_error("config"));
    assert_eq!(exec(&s, &[b"CONFIG", b"GET"]), arity_error("config|get"));
    assert_eq!(
        exec(&s, &[b"CONFIG", b"FOO"]),
        b"-ERR unknown subcommand 'FOO'. Try CONFIG HELP.\r\n"
    );
    let help = exec(&s, &[b"CONFIG", b"HELP"]);
    assert!(help.starts_with(b"*5\r\n+CONFIG <subcommand> [<arg> [value] [opt] ...]."));
}

/// Far in the future, as a unix time in milliseconds (2100-01-01)
const FUTURE_MS: i64 = 4_102_444_800_000;

fn integer(reply: &[u8]) -> i64 {
    assert!(
        reply.starts_with(b":"),
        "{:?}",
        String::from_utf8_lossy(reply)
    );
    std::str::from_utf8(&reply[1..reply.len() - 2])
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn expire_sets_a_relative_expiry_that_ttl_reports() {
    let s = shard();
    exec(&s, &[b"SET", b"k", b"v"]);
    assert_eq!(exec(&s, &[b"EXPIRE", b"k", b"100"]), b":1\r\n");
    assert_eq!(exec(&s, &[b"TTL", b"k"]), b":100\r\n");
    let pttl = integer(&exec(&s, &[b"PTTL", b"k"]));
    assert!((99_000..=100_000).contains(&pttl), "{pttl}");
    assert_eq!(exec(&s, &[b"PEXPIRE", b"k", b"5000"]), b":1\r\n");
    assert_eq!(exec(&s, &[b"TTL", b"k"]), b":5\r\n");
}

#[test]
fn ttl_family_reports_missing_keys_and_keys_without_expiry() {
    let s = shard();
    exec(&s, &[b"SET", b"p", b"v"]);
    for command in [&b"TTL"[..], b"PTTL", b"EXPIRETIME", b"PEXPIRETIME"] {
        assert_eq!(exec(&s, &[command, b"missing"]), b":-2\r\n");
        assert_eq!(exec(&s, &[command, b"p"]), b":-1\r\n");
    }
    exec(&s, &[b"EXPIREAT", b"p", b"4102444800"]);
    assert_eq!(exec(&s, &[b"EXPIRETIME", b"p"]), b":4102444800\r\n");
    assert_eq!(exec(&s, &[b"PEXPIRETIME", b"p"]), b":4102444800000\r\n");
}

#[test]
fn a_time_that_has_passed_deletes_the_key() {
    let s = shard();
    for args in [
        &[&b"EXPIRE"[..], b"k", b"0"][..],
        &[b"EXPIRE", b"k", b"-5"],
        &[b"PEXPIRE", b"k", b"0"],
        &[b"PEXPIREAT", b"k", b"1"],
        &[b"EXPIREAT", b"k", b"100"],
    ] {
        exec(&s, &[b"SET", b"k", b"v"]);
        assert_eq!(exec(&s, args), b":1\r\n");
        assert_eq!(exec(&s, &[b"EXISTS", b"k"]), b":0\r\n");
    }
}

#[test]
fn expire_and_persist_on_missing_keys_and_keys_without_expiry() {
    let s = shard();
    assert_eq!(exec(&s, &[b"EXPIRE", b"missing", b"10"]), b":0\r\n");
    assert_eq!(exec(&s, &[b"PERSIST", b"missing"]), b":0\r\n");
    exec(&s, &[b"SET", b"p", b"v"]);
    assert_eq!(exec(&s, &[b"PERSIST", b"p"]), b":0\r\n");
    exec(&s, &[b"EXPIRE", b"p", b"10"]);
    assert_eq!(exec(&s, &[b"PERSIST", b"p"]), b":1\r\n");
    assert_eq!(exec(&s, &[b"TTL", b"p"]), b":-1\r\n");
}

#[test]
fn expire_options_follow_redis() {
    let s = shard();
    let at = |offset: i64| (FUTURE_MS + offset).to_string().into_bytes();
    let set = |time: &[u8], option: &[u8]| integer(&exec(&s, &[b"PEXPIREAT", b"n", time, option]));
    exec(&s, &[b"SET", b"n", b"v"]);
    // Without an expiry: XX and GT fail, LT and NX set it
    assert_eq!(set(&at(0), b"XX"), 0);
    assert_eq!(set(&at(0), b"GT"), 0);
    assert_eq!(set(&at(0), b"LT"), 1);
    exec(&s, &[b"PERSIST", b"n"]);
    assert_eq!(set(&at(0), b"NX"), 1);
    // With an expiry: NX fails, XX sets, GT and LT compare
    assert_eq!(set(&at(10), b"NX"), 0);
    assert_eq!(set(&at(10), b"XX"), 1);
    assert_eq!(set(&at(5), b"GT"), 0);
    assert_eq!(set(&at(10), b"GT"), 0);
    assert_eq!(set(&at(50), b"gt"), 1);
    assert_eq!(set(&at(100), b"LT"), 0);
    assert_eq!(set(&at(1), b"LT"), 1);
    assert_eq!(integer(&exec(&s, &[b"PEXPIRETIME", b"n"])), FUTURE_MS + 1);
    // XX may be combined with GT or LT
    let reply = exec(&s, &[b"PEXPIREAT", b"n", &at(2), b"XX", b"GT"]);
    assert_eq!(integer(&reply), 1);
}

#[test]
fn incr_keeps_the_expiry_while_set_mset_and_rename_follow_redis() {
    let s = shard();
    let future = FUTURE_MS.to_string().into_bytes();
    exec(&s, &[b"SET", b"i", b"5"]);
    exec(&s, &[b"PEXPIREAT", b"i", &future]);
    assert_eq!(exec(&s, &[b"INCR", b"i"]), b":6\r\n");
    assert_eq!(integer(&exec(&s, &[b"PEXPIRETIME", b"i"])), FUTURE_MS);
    exec(&s, &[b"SET", b"i", b"7"]);
    assert_eq!(exec(&s, &[b"TTL", b"i"]), b":-1\r\n");
    exec(&s, &[b"PEXPIREAT", b"i", &future]);
    exec(&s, &[b"MSET", b"i", b"8"]);
    assert_eq!(exec(&s, &[b"TTL", b"i"]), b":-1\r\n");
    // RENAME moves the expiry and replaces the target's
    exec(&s, &[b"SET", b"r", b"v"]);
    exec(&s, &[b"PEXPIREAT", b"r", &future]);
    exec(&s, &[b"RENAME", b"r", b"r2"]);
    assert_eq!(integer(&exec(&s, &[b"PEXPIRETIME", b"r2"])), FUTURE_MS);
    exec(&s, &[b"SET", b"src", b"y"]);
    exec(&s, &[b"RENAME", b"src", b"r2"]);
    assert_eq!(exec(&s, &[b"TTL", b"r2"]), b":-1\r\n");
}

#[test]
fn expire_errors_match_redis() {
    let s = shard();
    exec(&s, &[b"SET", b"p", b"v"]);
    let cases: [(&[&[u8]], &[u8]); 11] = [
        (
            &[b"EXPIRE", b"p", b"abc"],
            b"-ERR value is not an integer or out of range\r\n",
        ),
        (
            &[b"EXPIRE", b"p", b"10", b"FOO"],
            b"-ERR Unsupported option FOO\r\n",
        ),
        // Options are checked before the time
        (
            &[b"EXPIRE", b"p", b"abc", b"FOO"],
            b"-ERR Unsupported option FOO\r\n",
        ),
        (
            &[b"EXPIRE", b"p", b"10", b"NX", b"XX"],
            b"-ERR NX and XX, GT or LT options at the same time are not compatible\r\n",
        ),
        (
            &[b"EXPIRE", b"p", b"10", b"NX", b"GT"],
            b"-ERR NX and XX, GT or LT options at the same time are not compatible\r\n",
        ),
        (
            &[b"EXPIRE", b"p", b"10", b"GT", b"LT"],
            b"-ERR GT and LT options at the same time are not compatible\r\n",
        ),
        (
            &[b"EXPIRE", b"p", b"9223372036854775807"],
            b"-ERR invalid expire time in 'expire' command\r\n",
        ),
        (
            &[b"EXPIRE", b"p", b"-9223372036854775808"],
            b"-ERR invalid expire time in 'expire' command\r\n",
        ),
        (
            &[b"PEXPIRE", b"p", b"9223372036854775807"],
            b"-ERR invalid expire time in 'pexpire' command\r\n",
        ),
        (
            &[b"EXPIREAT", b"p", b"9223372036854775807"],
            b"-ERR invalid expire time in 'expireat' command\r\n",
        ),
        (
            &[b"EXPIRE", b"p"],
            b"-ERR wrong number of arguments for 'expire' command\r\n",
        ),
    ];
    for (args, expected) in cases {
        assert_eq!(exec(&s, args), expected, "{args:?}");
    }
    for name in ["ttl", "pttl", "expiretime", "pexpiretime", "persist"] {
        let upper = name.to_uppercase();
        assert_eq!(exec(&s, &[upper.as_bytes()]), arity_error(name));
    }
    // The key is untouched by the failed commands
    assert_eq!(exec(&s, &[b"TTL", b"p"]), b":-1\r\n");
}

#[test]
fn info_keyspace_counts_keys_with_an_expiry() {
    let s = shard();
    exec(&s, &[b"MSET", b"a", b"1", b"b", b"2"]);
    exec(&s, &[b"EXPIRE", b"a", b"100"]);
    assert_eq!(
        info_text(&s, &[b"INFO", b"keyspace"]),
        "# Keyspace\r\ndb0:keys=2,expires=1,avg_ttl=0\r\n"
    );
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
fn set_nx_xx_and_get_follow_redis() {
    let s = shard();
    exec(&s, &[b"SET", b"b", b"v1"]);
    // NX with GET replies with the old value and leaves the key
    assert_eq!(
        exec(&s, &[b"SET", b"b", b"v2", b"NX", b"GET"]),
        b"$2\r\nv1\r\n"
    );
    assert_eq!(exec(&s, &[b"GET", b"b"]), b"$2\r\nv1\r\n");
    assert_eq!(exec(&s, &[b"SET", b"c", b"v", b"nx", b"get"]), b"$-1\r\n");
    assert_eq!(exec(&s, &[b"GET", b"c"]), b"$1\r\nv\r\n");
    assert_eq!(exec(&s, &[b"SET", b"d", b"v", b"XX"]), b"$-1\r\n");
    assert_eq!(exec(&s, &[b"EXISTS", b"d"]), b":0\r\n");
    assert_eq!(
        exec(&s, &[b"SET", b"b", b"v3", b"XX", b"GET"]),
        b"$2\r\nv1\r\n"
    );
    assert_eq!(exec(&s, &[b"GET", b"b"]), b"$2\r\nv3\r\n");
    assert_eq!(exec(&s, &[b"SET", b"b", b"v4", b"NX"]), b"$-1\r\n");
    assert_eq!(exec(&s, &[b"SET", b"e", b"v", b"NX", b"NX"]), b"+OK\r\n");
}

#[test]
fn set_expiry_options_follow_redis() {
    let s = shard();
    exec(&s, &[b"SET", b"k", b"v", b"EX", b"10"]);
    assert_eq!(exec(&s, &[b"TTL", b"k"]), b":10\r\n");
    exec(&s, &[b"SET", b"k", b"v2", b"KEEPTTL"]);
    assert_eq!(exec(&s, &[b"TTL", b"k"]), b":10\r\n");
    exec(&s, &[b"SET", b"k", b"v3"]);
    assert_eq!(exec(&s, &[b"TTL", b"k"]), b":-1\r\n");
    exec(&s, &[b"SET", b"k", b"v", b"PX", b"5000"]);
    assert_eq!(exec(&s, &[b"TTL", b"k"]), b":5\r\n");
    exec(&s, &[b"SET", b"k", b"v", b"EXAT", b"4102444800"]);
    assert_eq!(exec(&s, &[b"EXPIRETIME", b"k"]), b":4102444800\r\n");
    exec(&s, &[b"SET", b"k", b"v", b"PXAT", b"4102444800123"]);
    assert_eq!(exec(&s, &[b"PEXPIRETIME", b"k"]), b":4102444800123\r\n");
    // A repeated time option is fine; the last one counts
    exec(&s, &[b"SET", b"k", b"v", b"EX", b"10", b"EX", b"20"]);
    assert_eq!(exec(&s, &[b"TTL", b"k"]), b":20\r\n");
    // A time that has passed makes the new value expire at once
    assert_eq!(exec(&s, &[b"SET", b"a", b"v", b"PXAT", b"1"]), b"+OK\r\n");
    assert_eq!(exec(&s, &[b"EXISTS", b"a"]), b":0\r\n");
}

#[test]
fn set_option_errors_match_redis() {
    let s = shard();
    let syntax = b"-ERR syntax error\r\n";
    let invalid = b"-ERR invalid expire time in 'set' command\r\n";
    let cases: [(&[&[u8]], &[u8]); 14] = [
        (&[b"SET", b"k", b"v", b"NX", b"XX"], syntax),
        (&[b"SET", b"k", b"v", b"EX", b"10", b"PX", b"100"], syntax),
        (&[b"SET", b"k", b"v", b"EX", b"10", b"KEEPTTL"], syntax),
        (&[b"SET", b"k", b"v", b"KEEPTTL", b"EX", b"1"], syntax),
        (&[b"SET", b"k", b"v", b"EX"], syntax),
        (&[b"SET", b"k", b"v", b"FOO"], syntax),
        (&[b"SET", b"k", b"v", b"PERSIST"], syntax),
        // Options are checked before the time
        (&[b"SET", b"k", b"v", b"EX", b"abc", b"FOO"], syntax),
        (
            &[b"SET", b"k", b"v", b"EX", b"abc"],
            b"-ERR value is not an integer or out of range\r\n",
        ),
        (&[b"SET", b"k", b"v", b"EX", b"0"], invalid),
        (&[b"SET", b"k", b"v", b"PX", b"-1"], invalid),
        (&[b"SET", b"k", b"v", b"EXAT", b"0"], invalid),
        (&[b"SET", b"k", b"v", b"EX", b"9223372036854775"], invalid),
        (&[b"SET", b"k", b"v", b"EX", b"9223372036854776"], invalid),
    ];
    for (args, expected) in cases {
        assert_eq!(exec(&s, args), expected, "{args:?}");
    }
    // None of them wrote the key
    assert_eq!(exec(&s, &[b"EXISTS", b"k"]), b":0\r\n");
}

#[test]
fn setex_psetex_setnx_getset_and_getdel() {
    let s = shard();
    assert_eq!(exec(&s, &[b"SETEX", b"k", b"10", b"v"]), b"+OK\r\n");
    assert_eq!(exec(&s, &[b"TTL", b"k"]), b":10\r\n");
    assert_eq!(exec(&s, &[b"PSETEX", b"k", b"5000", b"v"]), b"+OK\r\n");
    assert_eq!(exec(&s, &[b"TTL", b"k"]), b":5\r\n");
    assert_eq!(
        exec(&s, &[b"SETEX", b"k", b"0", b"v"]),
        b"-ERR invalid expire time in 'setex' command\r\n"
    );
    assert_eq!(
        exec(&s, &[b"PSETEX", b"k", b"-5", b"v"]),
        b"-ERR invalid expire time in 'psetex' command\r\n"
    );
    assert_eq!(
        exec(&s, &[b"SETEX", b"k", b"abc", b"v"]),
        b"-ERR value is not an integer or out of range\r\n"
    );
    assert_eq!(exec(&s, &[b"SETNX", b"n", b"1"]), b":1\r\n");
    assert_eq!(exec(&s, &[b"SETNX", b"n", b"2"]), b":0\r\n");
    assert_eq!(exec(&s, &[b"GETSET", b"n", b"3"]), b"$1\r\n1\r\n");
    assert_eq!(exec(&s, &[b"GETSET", b"newkey", b"x"]), b"$-1\r\n");
    // GETSET removes the expiry, like SET
    assert_eq!(exec(&s, &[b"GETSET", b"k", b"w"]), b"$1\r\nv\r\n");
    assert_eq!(exec(&s, &[b"TTL", b"k"]), b":-1\r\n");
    assert_eq!(exec(&s, &[b"GETDEL", b"n"]), b"$1\r\n3\r\n");
    assert_eq!(exec(&s, &[b"GETDEL", b"n"]), b"$-1\r\n");
    for name in ["getdel", "getex", "setnx", "getset", "setex"] {
        let upper = name.to_uppercase();
        assert_eq!(exec(&s, &[upper.as_bytes()]), arity_error(name));
    }
}

#[test]
fn getex_reads_and_changes_the_expiry() {
    let s = shard();
    exec(&s, &[b"SET", b"g", b"v"]);
    assert_eq!(exec(&s, &[b"GETEX", b"g", b"EX", b"100"]), b"$1\r\nv\r\n");
    assert_eq!(exec(&s, &[b"TTL", b"g"]), b":100\r\n");
    assert_eq!(exec(&s, &[b"GETEX", b"g", b"PERSIST"]), b"$1\r\nv\r\n");
    assert_eq!(exec(&s, &[b"TTL", b"g"]), b":-1\r\n");
    assert_eq!(exec(&s, &[b"GETEX", b"g"]), b"$1\r\nv\r\n");
    assert_eq!(
        exec(&s, &[b"GETEX", b"g", b"EX", b"10", b"EX", b"20"]),
        b"$1\r\nv\r\n"
    );
    assert_eq!(exec(&s, &[b"TTL", b"g"]), b":20\r\n");
    // The time is only checked when the key exists
    assert_eq!(exec(&s, &[b"GETEX", b"missing", b"EX", b"abc"]), b"$-1\r\n");
    assert_eq!(
        exec(&s, &[b"GETEX", b"g", b"EX", b"abc"]),
        b"-ERR value is not an integer or out of range\r\n"
    );
    assert_eq!(
        exec(&s, &[b"GETEX", b"g", b"EX", b"0"]),
        b"-ERR invalid expire time in 'getex' command\r\n"
    );
    // The options are checked first
    for args in [
        &[&b"GETEX"[..], b"missing", b"NX"][..],
        &[b"GETEX", b"g", b"KEEPTTL"],
        &[b"GETEX", b"g", b"EX", b"10", b"PERSIST"],
    ] {
        assert_eq!(exec(&s, args), b"-ERR syntax error\r\n");
    }
    // An absolute time that has passed deletes the key after replying
    assert_eq!(exec(&s, &[b"GETEX", b"g", b"PXAT", b"1"]), b"$1\r\nv\r\n");
    assert_eq!(exec(&s, &[b"EXISTS", b"g"]), b":0\r\n");
}

#[test]
fn msetnx_sets_every_pair_only_if_no_key_exists() {
    let s = shard();
    assert_eq!(exec(&s, &[b"MSETNX", b"m1", b"1", b"m2", b"2"]), b":1\r\n");
    assert_eq!(exec(&s, &[b"MSETNX", b"m2", b"x", b"m3", b"3"]), b":0\r\n");
    assert_eq!(exec(&s, &[b"EXISTS", b"m3"]), b":0\r\n");
    assert_eq!(exec(&s, &[b"GET", b"m2"]), b"$1\r\n2\r\n");
    assert_eq!(exec(&s, &[b"MSETNX", b"m4"]), arity_error("msetnx"));
    assert_eq!(
        exec(&s, &[b"MSETNX", b"m4", b"1", b"m5"]),
        arity_error("msetnx")
    );
}

#[test]
fn concurrent_msetnx_with_a_shared_key_has_one_winner() {
    for round in 0..200 {
        let s = std::sync::Arc::new(shard());
        let shared = format!("shared:{round}");
        let replies: Vec<Vec<u8>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|t| {
                    let (s, shared) = (&s, &shared);
                    scope.spawn(move || {
                        let own = format!("own:{t}");
                        exec(
                            s,
                            &[b"MSETNX", own.as_bytes(), b"1", shared.as_bytes(), b"1"],
                        )
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let winners = replies.iter().filter(|r| r.as_slice() == b":1\r\n").count();
        assert_eq!(winners, 1, "round {round}: {replies:?}");
    }
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

const NOAUTH: &[u8] = b"-NOAUTH Authentication required.\r\n";
const WRONGPASS: &[u8] = b"-WRONGPASS invalid username-password pair or user is disabled.\r\n";

#[test]
fn a_session_with_a_password_runs_commands_only_after_auth() {
    let s = shard();
    let mut session = Session::with_password(b"secret");
    assert!(!session.is_authenticated());
    assert_eq!(exec_in(&s, &mut session, &[b"SET", b"k", b"v"]), NOAUTH);
    assert_eq!(exec_in(&s, &mut session, &[b"PING"]), NOAUTH);
    assert_eq!(exec_in(&s, &mut session, &[b"AUTH", b"wrong"]), WRONGPASS);
    assert_eq!(
        exec_in(&s, &mut session, &[b"AUTH", b"other", b"secret"]),
        WRONGPASS
    );
    assert_eq!(exec_in(&s, &mut session, &[b"AUTH", b"secret"]), b"+OK\r\n");
    assert!(session.is_authenticated());
    assert_eq!(exec_in(&s, &mut session, &[b"SET", b"k", b"v"]), b"+OK\r\n");
    // A failed AUTH afterwards keeps the connection authenticated
    assert_eq!(exec_in(&s, &mut session, &[b"AUTH", b"wrong"]), WRONGPASS);
    assert_eq!(exec_in(&s, &mut session, &[b"GET", b"k"]), b"$1\r\nv\r\n");
    assert_eq!(
        exec_in(&s, &mut session, &[b"AUTH", b"default", b"secret"]),
        b"+OK\r\n"
    );
    // Only the command that was refused went unexecuted
    assert_eq!(s.dict.len(), 1);
}

#[test]
fn quit_needs_no_auth() {
    let s = shard();
    let mut session = Session::with_password(b"secret");
    assert_eq!(exec_in(&s, &mut session, &[b"QUIT"]), b"+OK\r\n");
    assert!(session.is_closing());
}

#[test]
fn hello_authenticates_or_refuses_like_redis() {
    let s = shard();
    let noauth = b"-NOAUTH HELLO must be called with the client already authenticated, \
        otherwise the HELLO AUTH <user> <pass> option can be used to authenticate the client \
        and select the RESP protocol version at the same time\r\n";
    let mut session = Session::with_password(b"secret");
    assert_eq!(exec_in(&s, &mut session, &[b"HELLO"]), noauth);
    // Version and option errors come first
    assert_eq!(
        exec_in(&s, &mut session, &[b"HELLO", b"4"]),
        b"-NOPROTO unsupported protocol version\r\n"
    );
    assert_eq!(
        exec_in(&s, &mut session, &[b"HELLO", b"3", b"AUTH", b"default"]),
        b"-ERR Syntax error in HELLO option 'AUTH'\r\n"
    );
    // SETNAME is applied before the refusal, as in Redis
    assert_eq!(
        exec_in(&s, &mut session, &[b"HELLO", b"3", b"SETNAME", b"early"]),
        noauth
    );
    assert_eq!(session.protocol(), Protocol::Resp2);
    assert_eq!(session.name().map(|n| &n[..]), Some(&b"early"[..]));
    assert_eq!(
        exec_in(
            &s,
            &mut session,
            &[b"HELLO", b"3", b"AUTH", b"default", b"wrong"]
        ),
        WRONGPASS
    );
    let id = session.id();
    assert_eq!(
        exec_in(
            &s,
            &mut session,
            &[b"HELLO", b"3", b"AUTH", b"default", b"secret"]
        ),
        hello_reply(3, id)
    );
    assert!(session.is_authenticated());
    assert_eq!(exec_in(&s, &mut session, &[b"GET", b"k"]), b"_\r\n");
}

#[test]
fn auth_without_a_password_set_follows_redis() {
    let s = shard();
    let mut session = Session::new();
    assert_eq!(
        exec_in(&s, &mut session, &[b"AUTH", b"x"]),
        b"-ERR AUTH <password> called without any password configured for the default user. \
          Are you sure your configuration is correct?\r\n"
    );
    assert_eq!(
        exec_in(&s, &mut session, &[b"AUTH", b"default", b"x"]),
        b"+OK\r\n"
    );
    assert_eq!(
        exec_in(&s, &mut session, &[b"AUTH", b"other", b"x"]),
        WRONGPASS
    );
    assert_eq!(
        exec_in(&s, &mut session, &[b"AUTH", b"a", b"b", b"c"]),
        b"-ERR syntax error\r\n"
    );
    assert_eq!(
        exec(&s, &[b"AUTH"]),
        b"-ERR wrong number of arguments for 'auth' command\r\n"
    );
}

#[test]
fn command_info_describes_commands_like_redis_7() {
    let s = shard();
    // Redis 7.0's reply, but without key specifications
    let get = "*10\r\n$3\r\nget\r\n:2\r\n*2\r\n+readonly\r\n+fast\r\n:1\r\n:1\r\n:1\r\n\
               *3\r\n+@read\r\n+@string\r\n+@fast\r\n*0\r\n*0\r\n*0\r\n";
    assert_eq!(
        String::from_utf8(exec(&s, &[b"COMMAND", b"INFO", b"get", b"nosuch"])).unwrap(),
        format!("*2\r\n{get}$-1\r\n")
    );
    let mset = exec(&s, &[b"COMMAND", b"INFO", b"MSET"]);
    assert!(mset.starts_with(
        b"*1\r\n*10\r\n$4\r\nmset\r\n:-3\r\n*2\r\n+write\r\n+denyoom\r\n:1\r\n:-1\r\n:2\r\n"
    ));
    assert!(mset.ends_with(
        concat!(
            "*2\r\n$26\r\nrequest_policy:multi_shard\r\n",
            "$29\r\nresponse_policy:all_succeeded\r\n*0\r\n*0\r\n"
        )
        .as_bytes()
    ));
    // A subcommand by its full name, and among the container's details
    let config_get = "*10\r\n$10\r\nconfig|get\r\n:-3\r\n*4\r\n+admin\r\n+noscript\r\n+loading\r\n\
                      +stale\r\n:0\r\n:0\r\n:0\r\n*3\r\n+@admin\r\n+@slow\r\n+@dangerous\r\n\
                      *0\r\n*0\r\n*0\r\n";
    assert_eq!(
        String::from_utf8(exec(
            &s,
            &[b"COMMAND", b"INFO", b"CONFIG|GET", b"config|nosuch"]
        ))
        .unwrap(),
        format!("*2\r\n{config_get}$-1\r\n")
    );
    let config = String::from_utf8(exec(&s, &[b"COMMAND", b"INFO", b"config"])).unwrap();
    assert!(config.contains(&format!("*2\r\n{config_get}")));

    // RESP3 lists flags, categories, tips, key specifications and
    // subcommands as sets
    let mut session = Session::new();
    exec_in(&s, &mut session, &[b"HELLO", b"3"]);
    assert_eq!(
        exec_in(&s, &mut session, &[b"COMMAND", b"INFO", b"get", b"x"]),
        "*2\r\n*10\r\n$3\r\nget\r\n:2\r\n~2\r\n+readonly\r\n+fast\r\n:1\r\n:1\r\n:1\r\n\
         ~3\r\n+@read\r\n+@string\r\n+@fast\r\n~0\r\n~0\r\n~0\r\n_\r\n"
            .as_bytes()
    );

    // COMMAND and COMMAND INFO without names describe every command
    let count = integer(&exec(&s, &[b"COMMAND", b"COUNT"]));
    let all = exec(&s, &[b"COMMAND"]);
    assert!(all.starts_with(format!("*{count}\r\n*10\r\n").as_bytes()));
    assert_eq!(exec(&s, &[b"COMMAND", b"INFO"]), all);
}

/// The bulk strings of an array reply, as text
fn strings(reply: &[u8]) -> Vec<String> {
    bulk_strings(reply)
        .into_iter()
        .map(|s| String::from_utf8(s).unwrap())
        .collect()
}

#[test]
fn command_list_and_getkeys_follow_redis() {
    let s = shard();
    let list = |args: &[&[u8]]| {
        let mut names = strings(&exec(&s, args));
        names.sort();
        names
    };
    assert_eq!(
        list(&[b"COMMAND", b"LIST", b"FILTERBY", b"PATTERN", b"GET*"]),
        ["get", "getdel", "getex", "getset"]
    );
    assert_eq!(
        list(&[b"COMMAND", b"LIST", b"FILTERBY", b"pattern", b"config*"]),
        ["config", "config|get", "config|help"]
    );
    assert_eq!(
        list(&[b"COMMAND", b"LIST", b"FILTERBY", b"ACLCAT", b"Dangerous"]),
        ["config|get", "flushall", "flushdb", "info", "keys"]
    );
    assert_eq!(
        exec(&s, &[b"COMMAND", b"LIST", b"FILTERBY", b"MODULE", b"x"]),
        b"*0\r\n"
    );
    let every = list(&[b"COMMAND", b"LIST"]);
    assert!(every.contains(&"client|setinfo".to_string()) && every.contains(&"auth".to_string()));
    for args in [
        &[&b"COMMAND"[..], b"LIST", b"x"][..],
        &[b"COMMAND", b"LIST", b"FILTERBY", b"NOPE", b"x"],
    ] {
        assert_eq!(exec(&s, args), b"-ERR syntax error\r\n");
    }

    let keys = |args: &[&[u8]]| strings(&exec(&s, args));
    assert_eq!(
        keys(&[b"COMMAND", b"GETKEYS", b"MSET", b"a", b"1", b"b", b"2"]),
        ["a", "b"]
    );
    assert_eq!(
        keys(&[b"COMMAND", b"GETKEYS", b"set", b"k", b"v", b"EX", b"1"]),
        ["k"]
    );
    assert_eq!(
        keys(&[b"COMMAND", b"GETKEYS", b"DEL", b"a", b"b", b"c"]),
        ["a", "b", "c"]
    );
    assert_eq!(
        exec(&s, &[b"COMMAND", b"GETKEYS", b"CLIENT", b"ID"]),
        b"-ERR The command has no key arguments\r\n"
    );
    assert_eq!(
        exec(&s, &[b"COMMAND", b"GETKEYS", b"NOSUCH", b"x"]),
        b"-ERR Invalid command specified\r\n"
    );
    assert_eq!(
        exec(&s, &[b"COMMAND", b"GETKEYS", b"GET", b"a", b"b"]),
        b"-ERR Invalid number of arguments specified for command\r\n"
    );
    assert_eq!(
        exec(&s, &[b"COMMAND", b"GETKEYS", b"GET"]),
        b"-ERR wrong number of arguments for 'command|getkeys' command\r\n"
    );
}

#[test]
fn command_docs_is_left_to_redis_cli() {
    let s = shard();
    // Without DOCS, redis-cli falls back to its own command hints
    assert_eq!(
        exec(&s, &[b"COMMAND", b"DOCS", b"get"]),
        b"-ERR unknown subcommand 'DOCS'. Try COMMAND HELP.\r\n"
    );
    assert_eq!(
        exec(&s, &[b"COMMAND", b"COUNT", b"x"]),
        b"-ERR wrong number of arguments for 'command|count' command\r\n"
    );
    let help = exec(&s, &[b"COMMAND", b"HELP"]);
    assert!(help.starts_with(b"*15\r\n+COMMAND <subcommand>"));
}
