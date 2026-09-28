mod common;

use common::exec;
use ignix::*;

#[test]
fn set_get_del_cycle() {
    let shard = Shard::new(0, None);
    assert_eq!(exec(&shard, &[b"SET", b"a", b"1"]), b"+OK\r\n");
    assert_eq!(exec(&shard, &[b"GET", b"a"]), b"$1\r\n1\r\n");
    assert_eq!(exec(&shard, &[b"DEL", b"a"]), b":1\r\n");
    assert_eq!(exec(&shard, &[b"GET", b"a"]), b"$-1\r\n");
}

#[test]
fn rename_exists_incr() {
    let s = Shard::new(0, None);
    assert_eq!(exec(&s, &[b"SET", b"x", b"41"]), b"+OK\r\n");
    assert_eq!(exec(&s, &[b"EXISTS", b"x"]), b":1\r\n");
    assert_eq!(exec(&s, &[b"INCR", b"x"]), b":42\r\n");
    assert_eq!(exec(&s, &[b"RENAME", b"x", b"y"]), b"+OK\r\n");
    assert_eq!(exec(&s, &[b"GET", b"y"]), b"$2\r\n42\r\n");
}
