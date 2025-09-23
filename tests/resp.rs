use bytes::BytesMut;
use ignix::*;

#[test]
fn parse_ping_and_set_get() {
    let mut buf = BytesMut::new();
    buf.extend_from_slice(
        b"*1\r\n$4\r\nPING\r\n",
    );
    buf.extend_from_slice(
        b"*3\r\n$3\r\nSET\r\n$1\r\na\r\n$1\r\n1\r\n",
    );
    buf.extend_from_slice(
        b"*2\r\n$3\r\nGET\r\n$1\r\na\r\n",
    );
    let mut cmds = Vec::new();
    protocol::parse_many(&mut buf, &mut cmds).unwrap();
    assert!(matches!(cmds[0], Cmd::Ping));
    assert!(matches!(cmds[1], Cmd::Set(_, _)));
    assert!(matches!(cmds[2], Cmd::Get(_)));
}
