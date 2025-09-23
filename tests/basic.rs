use ignix::*;

#[test]
fn set_get_del_cycle() {
    let mut shard = Shard::new(0, None);
    assert_eq!(
        String::from_utf8_lossy(&shard.exec(Cmd::Set(b"a".to_vec(), b"1".to_vec()))),
        "+OK\r\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&shard.exec(Cmd::Get(b"a".to_vec()))),
        "$1\r\n1\r\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&shard.exec(Cmd::Del(b"a".to_vec()))),
        ":1\r\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&shard.exec(Cmd::Get(b"a".to_vec()))),
        "$-1\r\n"
    );
}

#[test]
fn rename_exists_incr() {
    let mut s = Shard::new(0, None);
    s.exec(Cmd::Set(b"x".to_vec(), b"41".to_vec()));
    assert_eq!(
        s.exec(Cmd::Exists(b"x".to_vec())),
        protocol::resp_integer(1)
    );
    assert_eq!(s.exec(Cmd::Incr(b"x".to_vec())), protocol::resp_integer(42));
    assert_eq!(
        s.exec(Cmd::Rename(b"x".to_vec(), b"y".to_vec())),
        protocol::resp_simple("OK")
    );
    assert_eq!(s.exec(Cmd::Get(b"y".to_vec())), protocol::resp_bulk(b"42"));
}
