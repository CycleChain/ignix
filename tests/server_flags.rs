//! The server binary's command line. These tests run the `ignix` binary
//! itself; the one that starts a server picks a free port of its own.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const IGNIX: &str = env!("CARGO_BIN_EXE_ignix");

#[test]
fn unknown_options_are_a_usage_error() {
    for args in [
        &["--nope"][..],
        &["--port", "0"],
        &["--port"],
        &["--bind", "nowhere"],
    ] {
        let output = Command::new(IGNIX).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.starts_with("ignix: "), "{args:?}: {stderr}");
        assert!(
            stderr.contains("Usage: ignix [options]"),
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn help_lists_the_options() {
    let output = Command::new(IGNIX).arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for option in [
        "--bind",
        "--port",
        "--requirepass",
        "--backend",
        "--busy-poll-us",
    ] {
        assert!(stdout.contains(option), "{option} missing from {stdout}");
    }
    let output = Command::new(IGNIX).arg("--version").output().unwrap();
    let version = format!("ignix {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(String::from_utf8_lossy(&output.stdout), version);
}

/// A server process, stopped and its directory removed when dropped
struct Server {
    child: Child,
    dir: std::path::PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Start the binary with `args` in a fresh directory, where it writes its
/// AOF, and wait until `port` accepts connections
fn start(args: &[&str], port: u16) -> Server {
    let dir = std::env::temp_dir().join(format!("ignix-flags-{}-{port}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let child = Command::new(IGNIX)
        .args(args)
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut server = Server { child, dir };
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            server.child.try_wait().unwrap().is_none(),
            "the server exited"
        );
        assert!(Instant::now() < deadline, "the server did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    server
}

fn req(args: &[&str]) -> Vec<u8> {
    let mut out = format!("*{}\r\n", args.len()).into_bytes();
    for arg in args {
        out.extend_from_slice(format!("${}\r\n{arg}\r\n", arg.len()).as_bytes());
    }
    out
}

/// Send `args` and read one reply of at most `lines` lines
fn command(
    stream: &mut TcpStream,
    reader: &mut impl BufRead,
    args: &[&str],
    lines: usize,
) -> String {
    stream.write_all(&req(args)).unwrap();
    let mut reply = String::new();
    for _ in 0..lines {
        reader.read_line(&mut reply).unwrap();
    }
    reply
}

#[test]
#[ignore = "starts an ignix server on a free port"]
fn bind_port_and_requirepass_are_used() {
    // A port that was free a moment ago
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let port_arg = port.to_string();
    let _server = start(
        &[
            "--bind",
            "127.0.0.1",
            "--port",
            &port_arg,
            "--requirepass",
            "secret",
        ],
        port,
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    assert_eq!(
        command(&mut stream, &mut reader, &["PING"], 1),
        "-NOAUTH Authentication required.\r\n"
    );
    assert_eq!(
        command(&mut stream, &mut reader, &["AUTH", "secret"], 1),
        "+OK\r\n"
    );
    let expected = format!(
        "*4\r\n$4\r\nbind\r\n$9\r\n127.0.0.1\r\n$4\r\nport\r\n${}\r\n{port}\r\n",
        port_arg.len()
    );
    assert_eq!(
        command(
            &mut stream,
            &mut reader,
            &["CONFIG", "GET", "bind", "port"],
            9
        ),
        expected
    );
}
