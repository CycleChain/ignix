//! Helpers shared by the integration tests.
//!
//! Commands are built from raw RESP requests and turned into `Cmd` values by
//! `protocol::parse_one`, so the tests do not depend on the shape of the `Cmd`
//! variants.

#![allow(dead_code)]

use bytes::BytesMut;
use ignix::{protocol, Session, Shard};

/// Encode `args` as a RESP array of bulk strings, the way clients send commands.
pub fn req(args: &[&[u8]]) -> Vec<u8> {
    let mut out = format!("*{}\r\n", args.len()).into_bytes();
    for arg in args {
        out.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
        out.extend_from_slice(arg);
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// Parse exactly one request with `protocol::parse_one` and execute it on `shard`.
///
/// Returns the raw reply bytes. A parse error is rendered as a RESP error line
/// (`-<message>\r\n`), which is how the server answers such a request.
pub fn exec_resp(shard: &Shard, request: &[u8]) -> Vec<u8> {
    match protocol::parse_one(request) {
        Ok(Some((consumed, cmd))) => {
            assert_eq!(
                consumed,
                request.len(),
                "request must contain exactly one command"
            );
            let mut out = BytesMut::new();
            shard.exec(cmd, &mut out);
            out.to_vec()
        }
        Ok(None) => panic!("incomplete request: {:?}", String::from_utf8_lossy(request)),
        Err(e) => format!("-{}\r\n", e.to_string().replace(['\r', '\n'], " ")).into_bytes(),
    }
}

/// Build a request from `args` and execute it on `shard`.
pub fn exec(shard: &Shard, args: &[&[u8]]) -> Vec<u8> {
    exec_resp(shard, &req(args))
}

/// Build a request from `args` and execute it on `shard` for the connection
/// of `session`, so connection state such as the protocol carries over.
pub fn exec_in(shard: &Shard, session: &mut Session, args: &[&[u8]]) -> Vec<u8> {
    let request = req(args);
    match protocol::parse_one(&request) {
        Ok(Some((_, cmd))) => {
            let mut out = BytesMut::new();
            shard.exec_session(cmd, session, &mut out);
            out.to_vec()
        }
        Ok(None) => panic!("incomplete request"),
        Err(e) => format!("-{}\r\n", e.to_string().replace(['\r', '\n'], " ")).into_bytes(),
    }
}
