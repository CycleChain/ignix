use ignix::*;
use bytes::Bytes;

#[test]
fn set_get_del_cycle() {
    let shard = Shard::new(0, None);
    assert_eq!(
        String::from_utf8_lossy(&shard.exec(Cmd::Set(Bytes::from_static(b"a"), Bytes::from_static(b"1")))),
        "+OK\r\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&shard.exec(Cmd::Get(Bytes::from_static(b"a")))),
        "$1\r\n1\r\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&shard.exec(Cmd::Del(Bytes::from_static(b"a")))),
        ":1\r\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&shard.exec(Cmd::Get(Bytes::from_static(b"a")))),
        "$-1\r\n"
    );
}

#[test]
fn rename_exists_incr() {
    let s = Shard::new(0, None);
    s.exec(Cmd::Set(Bytes::from_static(b"x"), Bytes::from_static(b"41")));
    assert_eq!(
        s.exec(Cmd::Exists(Bytes::from_static(b"x"))),
        protocol::resp_integer(1)
    );
    assert_eq!(s.exec(Cmd::Incr(Bytes::from_static(b"x"))), protocol::resp_integer(42));
    assert_eq!(
        s.exec(Cmd::Rename(Bytes::from_static(b"x"), Bytes::from_static(b"y"))),
        protocol::resp_simple("OK")
    );
    assert_eq!(s.exec(Cmd::Get(Bytes::from_static(b"y"))), protocol::resp_bulk(b"42"));
}
