/*!
 * INFO, CONFIG GET and COMMAND
 *
 * The server description, configuration and commands Ignix reports, in the
 * formats Redis uses, so clients and tools that read them keep working.
 */

use crate::commands::{self, CommandSpec, SubcommandSpec, COMMANDS};
use crate::glob::Pattern;
use crate::protocol::{
    write_array_len, write_bulk, write_error, write_integer, write_map_len, write_nil,
    write_set_len, write_simple, write_verbatim, CommandFilter, Protocol,
};
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
                keys => write!(
                    text,
                    "db0:keys={keys},expires={},avg_ttl=0\r\n",
                    shard.dict.count_volatile()
                ),
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

/// Write the description COMMAND INFO gives of a command or subcommand, as
/// Redis 7.0 does: name, arity, flags, first key, last key, key step, ACL
/// categories, tips, key specifications (Ignix reports none) and the
/// descriptions of its subcommands
#[allow(clippy::too_many_arguments)]
fn write_description(
    name: &str,
    arity: i32,
    flags: &[&str],
    (first, last, step): (i32, i32, i32),
    acl: &[&str],
    tips: &[&str],
    subcommands: &[SubcommandSpec],
    protocol: Protocol,
    out: &mut BytesMut,
) {
    write_array_len(10, out);
    write_bulk(name.as_bytes(), out);
    write_integer(i64::from(arity), out);
    write_set_len(protocol, flags.len(), out);
    for flag in flags {
        write_simple(flag, out);
    }
    for position in [first, last, step] {
        write_integer(i64::from(position), out);
    }
    write_set_len(protocol, acl.len(), out);
    for category in acl {
        write_simple(category, out);
    }
    write_set_len(protocol, tips.len(), out);
    for tip in tips {
        write_bulk(tip.as_bytes(), out);
    }
    write_set_len(protocol, 0, out);
    write_set_len(protocol, subcommands.len(), out);
    for sub in subcommands {
        write_subcommand(sub, protocol, out);
    }
}

fn write_command(spec: &CommandSpec, protocol: Protocol, out: &mut BytesMut) {
    let subcommands = commands::subcommands(spec.kind);
    write_description(
        spec.name,
        spec.arity,
        spec.flags,
        spec.keys,
        spec.acl,
        spec.tips,
        subcommands,
        protocol,
        out,
    );
}

fn write_subcommand(sub: &SubcommandSpec, protocol: Protocol, out: &mut BytesMut) {
    write_description(
        sub.name,
        sub.arity,
        sub.flags,
        (0, 0, 0),
        sub.acl,
        sub.tips,
        &[],
        protocol,
        out,
    );
}

/// Write the COMMAND INFO reply: the description of each command in
/// `names`, a subcommand being named `command|subcommand`, and nil for an
/// unknown one; every command's without names (as COMMAND does)
pub(crate) fn write_command_info(names: &[Bytes], protocol: Protocol, out: &mut BytesMut) {
    if names.is_empty() {
        write_array_len(COMMANDS.len(), out);
        for spec in COMMANDS {
            write_command(spec, protocol, out);
        }
        return;
    }
    write_array_len(names.len(), out);
    for name in names {
        let found = match name.iter().position(|&b| b == b'|') {
            None => commands::lookup(name).map(|spec| write_command(spec, protocol, out)),
            Some(bar) => commands::lookup(&name[..bar])
                .and_then(|spec| commands::find_subcommand(spec.kind, &name[bar + 1..]))
                .map(|sub| write_subcommand(sub, protocol, out)),
        };
        if found.is_none() {
            write_nil(protocol, out);
        }
    }
}

/// Write the COMMAND LIST reply: the name of every command and subcommand
/// that `filter` keeps
pub(crate) fn write_command_list(filter: Option<&CommandFilter>, out: &mut BytesMut) {
    let pattern = match filter {
        Some(CommandFilter::Pattern(pattern)) => Some(Pattern::new(pattern, true)),
        _ => None,
    };
    let keep = |name: &str, acl: &[&str]| match filter {
        None => true,
        Some(CommandFilter::Module(_)) => false,
        Some(CommandFilter::AclCat(category)) => acl
            .iter()
            .any(|c| c.as_bytes()[1..].eq_ignore_ascii_case(category)),
        Some(CommandFilter::Pattern(_)) => {
            pattern.as_ref().is_some_and(|p| p.matches(name.as_bytes()))
        }
    };
    let mut names = Vec::new();
    for spec in COMMANDS {
        if keep(spec.name, spec.acl) {
            names.push(spec.name);
        }
        for sub in commands::subcommands(spec.kind) {
            if keep(sub.name, sub.acl) {
                names.push(sub.name);
            }
        }
    }
    write_array_len(names.len(), out);
    for name in names {
        write_bulk(name.as_bytes(), out);
    }
}

/// Write the COMMAND GETKEYS reply: the key arguments of the command in
/// `args`, found from its first and last key positions and step
pub(crate) fn write_command_getkeys(args: &[Bytes], out: &mut BytesMut) {
    let Some(spec) = commands::lookup(&args[0]) else {
        return write_error("ERR Invalid command specified", out);
    };
    if !spec.arity_matches(args.len()) {
        return write_error("ERR Invalid number of arguments specified for command", out);
    }
    let (first, last, step) = spec.keys;
    if first == 0 {
        return write_error("ERR The command has no key arguments", out);
    }
    let argc = args.len() as i32;
    let last = if last < 0 {
        argc + last
    } else {
        last.min(argc - 1)
    };
    let positions: Vec<usize> = (first..=last)
        .step_by(step as usize)
        .map(|i| i as usize)
        .collect();
    write_array_len(positions.len(), out);
    for i in positions {
        write_bulk(&args[i], out);
    }
}
