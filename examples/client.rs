use std::io::{Read, Write};
use std::net::TcpStream;

fn main() {
    let mut s = TcpStream::connect("127.0.0.1:7379").expect("connect");
    // RESP ile: SET hello world
    let cmd = b"*3\r\n$3\r\nSET\r\n$5\r\nhello\r\n$5\r\nworld\r\n";
    s.write_all(cmd).unwrap();

    let mut buf = [0u8; 128];
    let n = s.read(&mut buf).unwrap();
    print!("{}", String::from_utf8_lossy(&buf[..n]));

    // GET hello
    let cmd = b"*2\r\n$3\r\nGET\r\n$5\r\nhello\r\n";
    s.write_all(cmd).unwrap();
    let n = s.read(&mut buf).unwrap();
    print!("{}", String::from_utf8_lossy(&buf[..n]));
}
