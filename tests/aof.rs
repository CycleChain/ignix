//! Append-only file records: binary-safe encoding, which commands are
//! logged, and how an unusable AOF path is reported.

mod common;

use bytes::Bytes;
use common::exec;
use ignix::{emit_aof_incr, emit_aof_mset, emit_aof_rename, emit_aof_set, spawn_aof_writer, Shard};
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
