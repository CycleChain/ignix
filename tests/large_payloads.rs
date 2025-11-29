use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn get_client() -> TcpStream {
    let stream = TcpStream::connect("127.0.0.1:7379").expect("Failed to connect");
    stream.set_read_timeout(Some(Duration::from_secs(30))).expect("Failed to set read timeout");
    stream.set_write_timeout(Some(Duration::from_secs(30))).expect("Failed to set write timeout");
    stream
}

fn send_cmd(stream: &mut TcpStream, cmd: &[u8]) -> Vec<u8> {
    stream.write_all(cmd).expect("Failed to write command");
    
    // Read response
    // Simple implementation: read until we get a complete RESP response
    // For this test, we assume the server behaves nicely
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).expect("Failed to read response");
    buf[..n].to_vec()
}

// Helper to read a large bulk string response
fn read_bulk_string(stream: &mut TcpStream) -> Vec<u8> {
    let mut reader = std::io::BufReader::new(stream);
    let mut line = String::new();
    std::io::BufRead::read_line(&mut reader, &mut line).expect("Failed to read header");
    
    if !line.starts_with('$') {
        panic!("Expected bulk string, got: {}", line);
    }
    
    let len: usize = line[1..].trim().parse().expect("Invalid length");
    let mut data = vec![0u8; len];
    std::io::Read::read_exact(&mut reader, &mut data).expect("Failed to read body");
    
    // Read trailing CRLF
    let mut crlf = [0u8; 2];
    std::io::Read::read_exact(&mut reader, &mut crlf).expect("Failed to read CRLF");
    
    data
}

#[test]
fn test_large_payload_100kb() {
    let mut stream = get_client();
    let size = 100 * 1024;
    let data = "x".repeat(size);
    let key = "large_100kb";
    
    // SET
    let cmd = format!("*3\r\n$3\r\nSET\r\n${}\r\n{}\r\n${}\r\n{}\r\n", key.len(), key, size, data);
    let resp = send_cmd(&mut stream, cmd.as_bytes());
    assert_eq!(resp, b"+OK\r\n");
    
    // GET
    let cmd = format!("*2\r\n$3\r\nGET\r\n${}\r\n{}\r\n", key.len(), key);
    stream.write_all(cmd.as_bytes()).expect("Failed to write GET");
    
    let received = read_bulk_string(&mut stream);
    assert_eq!(received.len(), size);
    assert_eq!(received, data.as_bytes());
}

#[test]
fn test_large_payload_1mb() {
    let mut stream = get_client();
    let size = 1024 * 1024;
    // Create random-ish data to avoid compression shortcuts if any (though Ignix doesn't compress)
    let data = "a".repeat(size); 
    let key = "large_1mb";
    
    // SET
    // We might need to write in chunks if the OS buffer is small, but TcpStream handles it
    let cmd_header = format!("*3\r\n$3\r\nSET\r\n${}\r\n{}\r\n${}\r\n", key.len(), key, size);
    stream.write_all(cmd_header.as_bytes()).expect("Failed to write header");
    stream.write_all(data.as_bytes()).expect("Failed to write data");
    stream.write_all(b"\r\n").expect("Failed to write CRLF");
    
    // Read simple string response +OK
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).expect("Failed to read SET response");
    assert_eq!(&buf[..n], b"+OK\r\n");
    
    // GET
    let cmd = format!("*2\r\n$3\r\nGET\r\n${}\r\n{}\r\n", key.len(), key);
    stream.write_all(cmd.as_bytes()).expect("Failed to write GET");
    
    let received = read_bulk_string(&mut stream);
    assert_eq!(received.len(), size);
    // Checking content might be slow, but let's check first and last bytes
    assert_eq!(received[0], b'a');
    assert_eq!(received[size-1], b'a');
}

#[test]
fn test_large_payload_10mb() {
    let mut stream = get_client();
    let size = 10 * 1024 * 1024;
    let key = "large_10mb";
    
    // SET
    let cmd_header = format!("*3\r\n$3\r\nSET\r\n${}\r\n{}\r\n${}\r\n", key.len(), key, size);
    stream.write_all(cmd_header.as_bytes()).expect("Failed to write header");
    
    // Write 10MB in chunks
    let chunk_size = 64 * 1024;
    let chunk = vec![b'z'; chunk_size];
    for _ in 0..(size / chunk_size) {
        stream.write_all(&chunk).expect("Failed to write chunk");
    }
    stream.write_all(b"\r\n").expect("Failed to write CRLF");
    
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).expect("Failed to read SET response");
    assert_eq!(&buf[..n], b"+OK\r\n");
    
    // GET
    let cmd = format!("*2\r\n$3\r\nGET\r\n${}\r\n{}\r\n", key.len(), key);
    stream.write_all(cmd.as_bytes()).expect("Failed to write GET");
    
    let received = read_bulk_string(&mut stream);
    assert_eq!(received.len(), size);
    assert_eq!(received[0], b'z');
}
