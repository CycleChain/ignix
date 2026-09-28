//! Append-only file records: binary-safe encoding, which commands are
//! logged, and how an unusable AOF path is reported.

mod common;

use bytes::Bytes;
use bytes::BytesMut;
use common::exec;
use ignix::{
    emit_aof_incr, emit_aof_mset, emit_aof_rename, emit_aof_set, parse_many, spawn_aof_writer, Cmd,
    Shard,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A fresh directory for one test's AOF file.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ignix-aof-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Wait until the AOF file contains `needle` and return its content.
///
/// The writer thread appends records in order, so once a marker record is
/// in the file every record sent before it is there too.
fn wait_for(path: &Path, needle: &[u8]) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let data = std::fs::read(path).unwrap_or_default();
        if contains(&data, needle) {
            return data;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {:?} in the AOF, content: {:?}",
            String::from_utf8_lossy(needle),
            String::from_utf8_lossy(&data)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Run `commands` on a shard that logs to a fresh AOF file, then return the
/// file content once a final marker record has been written.
fn aof_after(name: &str, commands: &[&[&[u8]]]) -> Vec<u8> {
    let dir = temp_dir(name);
    let path = dir.join("test.aof");
    let shard = Shard::new(0, Some(spawn_aof_writer(path.to_str().unwrap()).unwrap()));
    for args in commands {
        exec(&shard, args);
    }
    exec(&shard, &[b"SET", b"marker", b"end"]);
    let data = wait_for(&path, b"$6\r\nmarker\r\n$3\r\nend\r\n");
    drop(shard);
    let _ = std::fs::remove_dir_all(&dir);
    data
}

#[test]
fn aof_set_record_is_binary_safe() {
    assert_eq!(
        emit_aof_set(b"k", &[0xff, 0x00, 0xfe]),
        b"*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$3\r\n\xff\x00\xfe\r\n"
    );
}

#[test]
fn aof_rename_incr_and_mset_records_are_binary_safe() {
    assert_eq!(
        emit_aof_rename(b"\xffa", b"b"),
        b"*3\r\n$6\r\nRENAME\r\n$2\r\n\xffa\r\n$1\r\nb\r\n"
    );
    assert_eq!(
        emit_aof_incr(b"\xfe"),
        b"*2\r\n$4\r\nINCR\r\n$1\r\n\xfe\r\n"
    );
    let pairs = [(Bytes::from_static(b"\xff"), Bytes::from_static(b"\x80\x81"))];
    assert_eq!(
        emit_aof_mset(&pairs),
        b"*3\r\n$4\r\nMSET\r\n$1\r\n\xff\r\n$2\r\n\x80\x81\r\n"
    );
}

#[test]
fn del_is_logged_for_removed_keys_only() {
    let data = aof_after(
        "del",
        &[
            &[b"SET", b"a", b"1"],
            &[b"DEL", b"a", b"missing"],
            &[b"DEL", b"missing"],
        ],
    );
    assert!(
        contains(&data, b"*2\r\n$3\r\nDEL\r\n$1\r\na\r\n"),
        "DEL of an existing key must be logged: {:?}",
        String::from_utf8_lossy(&data)
    );
    assert!(
        !contains(&data, b"missing"),
        "keys that did not exist must not be logged: {:?}",
        String::from_utf8_lossy(&data)
    );
}

#[test]
fn unlink_is_logged_as_del_of_the_removed_keys() {
    let data = aof_after(
        "unlink",
        &[&[b"SET", b"u", b"1"], &[b"UNLINK", b"u", b"missing"]],
    );
    assert!(
        contains(&data, b"*2\r\n$3\r\nDEL\r\n$1\r\nu\r\n"),
        "{:?}",
        String::from_utf8_lossy(&data)
    );
    assert!(!contains(&data, b"UNLINK") && !contains(&data, b"missing"));
}

#[test]
fn flushdb_and_flushall_are_logged() {
    let data = aof_after(
        "flush",
        &[
            &[b"SET", b"f", b"1"],
            &[b"FLUSHDB", b"ASYNC"],
            &[b"FLUSHALL"],
        ],
    );
    let flushdb = b"*1\r\n$7\r\nFLUSHDB\r\n";
    let flushall = b"*1\r\n$8\r\nFLUSHALL\r\n";
    let position = |needle: &[u8]| data.windows(needle.len()).position(|w| w == needle);
    assert!(
        position(flushdb) < position(flushall) && position(flushdb).is_some(),
        "{:?}",
        String::from_utf8_lossy(&data)
    );
}

/// The PEXPIREAT record for `key` in `data`, and its time
fn pexpireat_time(data: &[u8], key: &[u8]) -> i64 {
    let head = [
        b"*3\r\n$9\r\nPEXPIREAT\r\n$".as_slice(),
        key.len().to_string().as_bytes(),
        b"\r\n",
        key,
        b"\r\n$",
    ]
    .concat();
    let start = data
        .windows(head.len())
        .position(|w| w == head.as_slice())
        .unwrap_or_else(|| {
            panic!(
                "no PEXPIREAT for {key:?} in {:?}",
                String::from_utf8_lossy(data)
            )
        })
        + head.len();
    let rest = &data[start..];
    let header_end = rest.windows(2).position(|w| w == b"\r\n").unwrap();
    let len: usize = std::str::from_utf8(&rest[..header_end])
        .unwrap()
        .parse()
        .unwrap();
    let value = &rest[header_end + 2..header_end + 2 + len];
    std::str::from_utf8(value).unwrap().parse().unwrap()
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[test]
fn expire_family_is_logged_as_pexpireat_with_an_absolute_time() {
    let before = unix_ms();
    let data = aof_after(
        "expire",
        &[
            &[b"SET", b"e", b"1"],
            &[b"EXPIRE", b"e", b"100"],
            &[b"SET", b"p", b"1"],
            &[b"PEXPIREAT", b"p", b"4102444800000"],
        ],
    );
    let after = unix_ms();
    let at = pexpireat_time(&data, b"e");
    assert!(before + 100_000 <= at && at <= after + 100_000, "{at}");
    assert_eq!(pexpireat_time(&data, b"p"), 4_102_444_800_000);
}

#[test]
fn passed_expiry_and_persist_are_logged_and_no_ops_are_not() {
    let data = aof_after(
        "expire-del",
        &[
            &[b"SET", b"gone", b"1"],
            &[b"EXPIRE", b"gone", b"0"],
            &[b"SET", b"kept", b"1"],
            &[b"EXPIRE", b"kept", b"100"],
            &[b"PERSIST", b"kept"],
            &[b"PERSIST", b"kept"],
            &[b"EXPIRE", b"missing", b"100"],
            &[b"EXPIRE", b"kept", b"100", b"XX"],
        ],
    );
    let text = String::from_utf8_lossy(&data);
    assert!(
        contains(&data, b"*2\r\n$3\r\nDEL\r\n$4\r\ngone\r\n"),
        "{text}"
    );
    assert_eq!(text.matches("PERSIST").count(), 1, "{text}");
    assert_eq!(text.matches("PEXPIREAT").count(), 1, "{text}");
    assert!(!contains(&data, b"missing"), "{text}");
}

/// The PXAT time of the `SET key value PXAT time` record for `key`
fn set_pxat_time(data: &[u8], key: &[u8], value: &[u8]) -> i64 {
    let head = [
        b"*5\r\n$3\r\nSET\r\n$".as_slice(),
        key.len().to_string().as_bytes(),
        b"\r\n",
        key,
        b"\r\n$",
        value.len().to_string().as_bytes(),
        b"\r\n",
        value,
        b"\r\n$4\r\nPXAT\r\n$",
    ]
    .concat();
    let start = data
        .windows(head.len())
        .position(|w| w == head.as_slice())
        .unwrap_or_else(|| {
            panic!(
                "no SET PXAT for {key:?}: {:?}",
                String::from_utf8_lossy(data)
            )
        })
        + head.len();
    let rest = &data[start..];
    let header_end = rest.windows(2).position(|w| w == b"\r\n").unwrap();
    let len: usize = std::str::from_utf8(&rest[..header_end])
        .unwrap()
        .parse()
        .unwrap();
    let time = &rest[header_end + 2..header_end + 2 + len];
    std::str::from_utf8(time).unwrap().parse().unwrap()
}

#[test]
fn set_with_an_expiry_is_logged_with_an_absolute_pxat() {
    let before = unix_ms();
    let data = aof_after(
        "set-pxat",
        &[
            &[b"SET", b"s", b"1", b"EX", b"100"],
            &[b"SETEX", b"x", b"100", b"2"],
            &[b"PSETEX", b"p", b"5000", b"3"],
            &[b"SET", b"a", b"4", b"PXAT", b"4102444800000"],
        ],
    );
    let after = unix_ms();
    let within = |key: &[u8], value: &[u8], offset: i64| {
        let at = set_pxat_time(&data, key, value);
        assert!(before + offset <= at && at <= after + offset, "{at}");
    };
    within(b"s", b"1", 100_000);
    within(b"x", b"2", 100_000);
    within(b"p", b"3", 5_000);
    assert_eq!(set_pxat_time(&data, b"a", b"4"), 4_102_444_800_000);
}

#[test]
fn set_family_logs_what_a_replay_needs() {
    let data = aof_after(
        "set-family",
        &[
            &[b"SET", b"k", b"1", b"KEEPTTL"],
            &[b"SET", b"k", b"2", b"NX"],
            &[b"SETNX", b"k", b"3"],
            &[b"SETNX", b"n", b"4"],
            &[b"GETSET", b"g", b"5"],
            &[b"GETDEL", b"g"],
            &[b"GETDEL", b"missing"],
            &[b"GETEX", b"n", b"PXAT", b"4102444800000"],
            &[b"GETEX", b"n", b"PERSIST"],
            &[b"GETEX", b"n", b"PXAT", b"1"],
            &[b"MSETNX", b"m1", b"6", b"m2", b"7"],
            &[b"MSETNX", b"m2", b"8", b"m3", b"9"],
        ],
    );
    let text = String::from_utf8_lossy(&data);
    let has = |record: &[u8]| contains(&data, record);
    assert!(
        has(b"*4\r\n$3\r\nSET\r\n$1\r\nk\r\n$1\r\n1\r\n$7\r\nKEEPTTL\r\n"),
        "{text}"
    );
    // Commands that did not set anything are not logged
    assert!(
        !has(b"$1\r\nk\r\n$1\r\n2\r\n") && !has(b"$1\r\nk\r\n$1\r\n3\r\n"),
        "{text}"
    );
    assert!(has(b"*3\r\n$3\r\nSET\r\n$1\r\nn\r\n$1\r\n4\r\n"), "{text}");
    assert!(has(b"*3\r\n$3\r\nSET\r\n$1\r\ng\r\n$1\r\n5\r\n"), "{text}");
    assert!(has(b"*2\r\n$3\r\nDEL\r\n$1\r\ng\r\n"), "{text}");
    assert!(!has(b"missing"), "{text}");
    assert_eq!(pexpireat_time(&data, b"n"), 4_102_444_800_000);
    assert!(has(b"*2\r\n$7\r\nPERSIST\r\n$1\r\nn\r\n"), "{text}");
    assert!(has(b"*2\r\n$3\r\nDEL\r\n$1\r\nn\r\n"), "{text}");
    assert!(
        has(b"*5\r\n$4\r\nMSET\r\n$2\r\nm1\r\n$1\r\n6\r\n$2\r\nm2\r\n$1\r\n7\r\n"),
        "{text}"
    );
    assert!(!has(b"$2\r\nm3\r\n"), "{text}");
}

#[test]
fn keys_removed_by_the_expiry_cycle_are_logged_as_del() {
    let dir = temp_dir("expire-cycle");
    let path = dir.join("test.aof");
    let shard = Shard::new(0, Some(spawn_aof_writer(path.to_str().unwrap()).unwrap()));
    // A time long past: the keys are stored already expired
    let gone: Vec<Vec<u8>> = (0..50).map(|i| format!("gone{i}").into_bytes()).collect();
    for key in &gone {
        exec(&shard, &[b"SET", key, b"v", b"PXAT", b"1"]);
    }
    exec(&shard, &[b"SET", b"kept", b"v", b"PX", b"3600000"]);
    exec(&shard, &[b"SET", b"plain", b"v"]);

    assert_eq!(shard.expire_cycle(Duration::from_secs(60)), gone.len());
    assert_eq!(shard.stats.expired_keys(), gone.len() as u64);
    assert_eq!(shard.dict.len(), 2);

    exec(&shard, &[b"SET", b"marker", b"end"]);
    let data = wait_for(&path, b"$6\r\nmarker\r\n$3\r\nend\r\n");
    drop(shard);
    let _ = std::fs::remove_dir_all(&dir);
    let mut buf = BytesMut::from(&data[..]);
    let mut cmds = Vec::new();
    parse_many(&mut buf, &mut cmds).expect("every record is a complete RESP command");
    // Each removed key is in exactly one DEL, after its SET
    let mut deleted = Vec::new();
    for cmd in &cmds {
        if let Cmd::Del(keys) = cmd {
            deleted.extend(keys.iter().map(|k| k.to_vec()));
        }
    }
    deleted.sort();
    let mut expected = gone.clone();
    expected.sort();
    assert_eq!(deleted, expected);
    let first_del = cmds.iter().position(|c| matches!(c, Cmd::Del(_))).unwrap();
    let last_set = cmds
        .iter()
        .rposition(|c| matches!(c, Cmd::SetWith(..)))
        .unwrap();
    assert!(last_set < first_del);
}

#[test]
fn failed_incr_is_not_logged() {
    let data = aof_after("incr", &[&[b"SET", b"t", b"abc"], &[b"INCR", b"t"]]);
    assert!(
        !contains(&data, b"INCR"),
        "a failed INCR must not be logged: {:?}",
        String::from_utf8_lossy(&data)
    );
}

#[test]
fn failed_rename_is_not_logged() {
    let data = aof_after("rename", &[&[b"RENAME", b"nokey", b"other"]]);
    assert!(!contains(&data, b"RENAME"));
}

#[test]
fn unopenable_aof_path_returns_an_error() {
    let dir = temp_dir("unopenable");
    let path = dir.join("missing-directory").join("test.aof");
    let result = spawn_aof_writer(path.to_str().unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        result.is_err(),
        "an AOF file that cannot be opened must be reported to the caller"
    );
}

#[test]
fn incrby_and_decr_are_logged_with_their_increment() {
    let data = aof_after(
        "incrby",
        &[
            &[b"INCRBY", b"k", b"5"],
            &[b"DECR", b"k"],
            &[b"DECRBY", b"k", b"-2"],
        ],
    );
    assert!(contains(
        &data,
        b"*3\r\n$6\r\nINCRBY\r\n$1\r\nk\r\n$1\r\n5\r\n"
    ));
    assert!(contains(
        &data,
        b"*3\r\n$6\r\nINCRBY\r\n$1\r\nk\r\n$2\r\n-1\r\n"
    ));
    assert!(contains(
        &data,
        b"*3\r\n$6\r\nINCRBY\r\n$1\r\nk\r\n$1\r\n2\r\n"
    ));
}

/// Wait until the AOF file holds exactly `expected` bytes and return them.
fn wait_for_len(path: &Path, expected: usize) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let data = std::fs::read(path).unwrap_or_default();
        if data.len() >= expected {
            return data;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} bytes in the AOF, got {}",
            data.len()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn concurrent_writers_keep_their_order_and_records_stay_whole() {
    let dir = temp_dir("concurrent");
    let path = dir.join("test.aof");
    let aof = spawn_aof_writer(path.to_str().unwrap()).unwrap();
    let (threads, per_thread) = (4, 5000);
    std::thread::scope(|scope| {
        for t in 0..threads {
            let aof = aof.clone();
            scope.spawn(move || {
                for i in 0..per_thread {
                    aof.write_owned(emit_aof_set(format!("t{t}:{i}").as_bytes(), b"value"));
                }
            });
        }
    });
    aof.write_owned(emit_aof_set(b"marker", b"end"));
    let data = wait_for(&path, b"$6\r\nmarker\r\n$3\r\nend\r\n");
    let _ = std::fs::remove_dir_all(&dir);

    let mut buf = BytesMut::from(&data[..]);
    let mut cmds = Vec::new();
    parse_many(&mut buf, &mut cmds).expect("every record is a complete RESP command");
    assert!(buf.is_empty(), "the file ends with a complete record");
    assert_eq!(cmds.len(), threads * per_thread + 1);
    let mut next = vec![0; threads];
    for cmd in &cmds[..cmds.len() - 1] {
        let Cmd::Set(key, _) = cmd else {
            panic!("unexpected record {cmd:?}")
        };
        let key = std::str::from_utf8(key).unwrap();
        let (t, i) = key[1..].split_once(':').unwrap();
        let (t, i): (usize, usize) = (t.parse().unwrap(), i.parse().unwrap());
        assert_eq!(i, next[t], "records of writer {t} are out of order");
        next[t] += 1;
    }
}

#[test]
fn large_record_keeps_its_place_between_small_ones() {
    let dir = temp_dir("large-record");
    let path = dir.join("test.aof");
    let aof = spawn_aof_writer(path.to_str().unwrap()).unwrap();
    let records = [
        emit_aof_set(b"before", b"1"),
        emit_aof_set(b"large", &vec![b'x'; 3 << 20]),
        emit_aof_set(b"after", b"2"),
    ];
    for record in &records {
        aof.write(record);
    }
    let expected = records.concat();
    let data = wait_for_len(&path, expected.len());
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        data == expected,
        "the records must be written in order, byte for byte"
    );
}

#[test]
fn records_sent_before_the_last_handle_is_dropped_are_written() {
    let dir = temp_dir("drop");
    let path = dir.join("test.aof");
    let aof = spawn_aof_writer(path.to_str().unwrap()).unwrap();
    let records: Vec<Vec<u8>> = (0..1000)
        .map(|i| emit_aof_set(format!("key{i}").as_bytes(), b"v"))
        .collect();
    for record in &records {
        aof.write(record);
    }
    drop(aof);
    let expected = records.concat();
    let data = wait_for_len(&path, expected.len());
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        data == expected,
        "every record sent before the drop must be written"
    );
}
