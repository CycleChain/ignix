/*!
 * INFO and CONFIG GET
 *
 * The server description and configuration Ignix reports, in the formats
 * Redis uses, so clients and tools that read them keep working.
 */

use crate::glob::Pattern;
use crate::protocol::{write_bulk, write_map_len, write_verbatim, Protocol};
use crate::shard::Shard;
use bytes::{Bytes, BytesMut};
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

/// The Redis version whose commands and replies Ignix follows, reported by
/// HELLO and INFO
pub(crate) const REDIS_VERSION: &str = "7.0.0";

/// INFO sections, in the order Redis writes them; all are default sections
const SECTIONS: [&str; 6] = [
    "server",
    "clients",
    "persistence",
    "stats",
    "replication",
    "keyspace",
];

/// The operating system name as `uname` reports it
fn os_name() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "Darwin",
        other => other,
    }
}

/// Write the INFO reply for `sections` (the default sections when empty).
/// Section names are matched ignoring case; unknown ones are left out.
pub(crate) fn write_info(
    shard: &Shard,
    sections: &[Bytes],
    protocol: Protocol,
    out: &mut BytesMut,
) {
    let all = sections.is_empty()
        || sections.iter().any(|s| {
            [&b"all"[..], b"default", b"everything"]
                .iter()
                .any(|name| s.eq_ignore_ascii_case(name))
        });
    let mut text = String::new();
    for name in SECTIONS {
        if !all
            && !sections
                .iter()
                .any(|s| s.eq_ignore_ascii_case(name.as_bytes()))
        {
            continue;
        }
        if !text.is_empty() {
            text.push_str("\r\n");
        }
        write_section(shard, name, &mut text);
    }
    write_verbatim(protocol, text.as_bytes(), out);
}

fn write_section(shard: &Shard, name: &str, text: &mut String) {
    let stats = &shard.stats;
    let listener = stats.listener();
    // Writing to a String cannot fail
    let _ = match name {
        "server" => {
            let uptime = stats.uptime().as_secs();
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_micros());
            let _ = write!(
                text,
                "# Server\r\nredis_version:{REDIS_VERSION}\r\nignix_version:{}\r\n\
                 redis_mode:standalone\r\nos:{} {}\r\narch_bits:{}\r\n",
                env!("CARGO_PKG_VERSION"),
                os_name(),
                std::env::consts::ARCH,
                usize::BITS,
            );
            if let Some(listener) = listener {
                let _ = write!(text, "multiplexing_api:{}\r\n", listener.api);
            }
            let _ = write!(text, "process_id:{}\r\n", std::process::id());
            if let Some(listener) = listener {
                let _ = write!(text, "tcp_port:{}\r\n", listener.addr.port());
            }
            write!(
                text,
                "server_time_usec:{now}\r\nuptime_in_seconds:{uptime}\r\nuptime_in_days:{}\r\n",
                uptime / 86_400
            )
        }
        "clients" => write!(
            text,
            "# Clients\r\nconnected_clients:{}\r\n",
            stats.connected_clients()
        ),
        "persistence" => write!(
            text,
            "# Persistence\r\nloading:0\r\naof_enabled:{}\r\n",
            u8::from(shard.aof.is_some())
        ),
        "stats" => write!(
            text,
            "# Stats\r\ntotal_connections_received:{}\r\ntotal_commands_processed:{}\r\n\
             expired_keys:{}\r\n",
            stats.connections_received(),
            stats.commands_processed(),
            stats.expired_keys()
        ),
        "replication" => write!(
            text,
            "# Replication\r\nrole:master\r\nconnected_slaves:0\r\n"
        ),
        _ => {
            text.push_str("# Keyspace\r\n");
            match shard.dict.len() {
                0 => Ok(()),
                keys => write!(text, "db0:keys={keys},expires=0,avg_ttl=0\r\n"),
            }
        }
    };
}

/// The configuration parameters CONFIG GET reports, with their values
fn parameters(shard: &Shard) -> [(&'static str, String); 12] {
    let listener = shard.stats.listener();
    [
        ("appendfsync", "everysec".into()),
        (
            "appendonly",
            if shard.aof.is_some() { "yes" } else { "no" }.into(),
        ),
        (
            "bind",
            listener
                .map(|l| l.addr.ip().to_string())
                .unwrap_or_default(),
        ),
        ("databases", "1".into()),
        (
            "dir",
            std::env::current_dir()
                .map(|dir| dir.display().to_string())
                .unwrap_or_default(),
        ),
        ("lazyfree-lazy-user-flush", "no".into()),
        ("maxmemory", "0".into()),
        ("maxmemory-policy", "noeviction".into()),
        ("port", listener.map_or(0, |l| l.addr.port()).to_string()),
        ("proto-max-bulk-len", "536870912".into()),
        ("save", String::new()),
        ("timeout", "0".into()),
    ]
}

/// Write the CONFIG GET reply for `patterns`. Like Redis, a name without
/// `*`, `?` or `[` is looked up ignoring case and reported as given; other
/// patterns are globs matched ignoring case. Each parameter appears once.
pub(crate) fn write_config_get(
    shard: &Shard,
    patterns: &[Bytes],
    protocol: Protocol,
    out: &mut BytesMut,
) {
    let parameters = parameters(shard);
    // Each matched parameter's index, and the name to report it under
    let mut matched: Vec<(usize, Bytes)> = Vec::new();
    for pattern in patterns {
        let is_glob = pattern.iter().any(|b| matches!(b, b'*' | b'?' | b'['));
        let glob = is_glob.then(|| Pattern::new(pattern, true));
        for (i, (name, _)) in parameters.iter().enumerate() {
            if matched.iter().any(|(j, _)| *j == i) {
                continue;
            }
            match &glob {
                Some(glob) if glob.matches(name.as_bytes()) => {
                    matched.push((i, Bytes::from_static(name.as_bytes())));
                }
                None if pattern.eq_ignore_ascii_case(name.as_bytes()) => {
                    matched.push((i, pattern.clone()));
                }
                _ => {}
            }
        }
    }
    write_map_len(protocol, matched.len(), out);
    for (i, name) in &matched {
        write_bulk(name, out);
        write_bulk(parameters[*i].1.as_bytes(), out);
    }
}
