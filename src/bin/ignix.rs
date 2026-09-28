/*!
 * Ignix Server Main Entry Point
 *
 * This is the main executable that starts the Ignix key-value server.
 * It parses the command line, initializes logging, creates the storage
 * shard, optionally enables AOF persistence, and starts the server loop.
 */

use anyhow::Result;
use ignix::*;
use std::ffi::OsString;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const USAGE: &str = "\
Usage: ignix [options]

Options:
  --bind ADDR          IPv4 or IPv6 address to listen on (default 0.0.0.0)
  --port N             TCP port to listen on (default 7379)
  --requirepass PASS   clients must authenticate with AUTH PASS first
  --backend uring      use the io_uring backend (Linux only; default mio)
  --busy-poll-us N     how long a worker of the default backend polls for
                       events before sleeping, in microseconds (default 50,
                       0 disables it)
  -h, --help           print this help
  -V, --version        print the version

Every option also takes the form --option=value. Writes are logged to
ignix.aof in the working directory. RUST_LOG sets the log level.
";

/// What the command line asks for
#[derive(Debug)]
enum Command {
    Run(Args),
    Help,
    Version,
}

/// How to run the server
#[derive(Debug)]
struct Args {
    /// Where to listen
    addr: SocketAddr,
    /// Whether to use the io_uring backend
    uring: bool,
    /// Options of the backend
    options: net::ServerOptions,
    /// Whether `--busy-poll-us` was given
    busy_poll_given: bool,
}

/// Parse the arguments that follow the program name
fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Command, String> {
    let mut ip = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    let mut port = 7379;
    let mut uring = false;
    let mut options = net::ServerOptions::default();
    let mut busy_poll_given = false;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let arg = arg
            .into_string()
            .map_err(|arg| format!("argument {arg:?} is not valid UTF-8"))?;
        if arg == "-h" || arg == "--help" {
            return Ok(Command::Help);
        }
        if arg == "-V" || arg == "--version" {
            return Ok(Command::Version);
        }
        // --option value or --option=value
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name.to_string(), Some(value.into())),
            _ => (arg, None),
        };
        if !matches!(
            name.as_str(),
            "--bind" | "--port" | "--requirepass" | "--backend" | "--busy-poll-us"
        ) {
            return Err(format!("unknown option '{name}'"));
        }
        let value = match inline.or_else(|| args.next()) {
            Some(value) => value
                .into_string()
                .map_err(|value| format!("value {value:?} of '{name}' is not valid UTF-8"))?,
            None => return Err(format!("option '{name}' needs a value")),
        };
        match name.as_str() {
            "--bind" => {
                ip = value.parse().map_err(|_| {
                    format!("invalid --bind address {value:?}: expected an IPv4 or IPv6 address")
                })?;
            }
            "--port" => {
                port = value
                    .parse()
                    .ok()
                    .filter(|&port: &u16| port != 0)
                    .ok_or_else(|| format!("invalid --port {value:?}: expected 1 to 65535"))?;
            }
            "--requirepass" => options.requirepass = Some(value),
            "--backend" => match value.as_str() {
                "uring" => uring = true,
                "mio" => uring = false,
                _ => {
                    return Err(format!(
                        "invalid --backend {value:?}: expected uring or mio"
                    ))
                }
            },
            _ => {
                let micros: u64 = value.parse().map_err(|_| {
                    format!("invalid --busy-poll-us {value:?}: expected a number of microseconds")
                })?;
                options.busy_poll = Duration::from_micros(micros);
                busy_poll_given = true;
            }
        }
    }
    Ok(Command::Run(Args {
        addr: SocketAddr::new(ip, port),
        uring,
        options,
        busy_poll_given,
    }))
}

/// Main function - entry point for Ignix server
///
/// Parses the command line (exiting with code 2 on a usage error), then:
/// 1. Initialize logging system
/// 2. Create AOF writer (if possible)
/// 3. Create storage shard
/// 4. Start server event loop
fn main() -> Result<()> {
    let args = match parse_args(std::env::args_os().skip(1)) {
        Ok(Command::Run(args)) => args,
        Ok(Command::Help) => {
            print!("{USAGE}");
            return Ok(());
        }
        Ok(Command::Version) => {
            println!("ignix {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Err(message) => {
            eprintln!("ignix: {message}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    // Initialize logging - respects RUST_LOG environment variable and shows
    // info and above by default. Example: RUST_LOG=debug cargo run --release
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    if args
        .options
        .requirepass
        .as_deref()
        .is_some_and(|p| !p.is_empty())
    {
        log::info!("clients must authenticate (requirepass is set)");
    }

    // Try to create AOF writer for persistence
    // If this fails, server will run without persistence (in-memory only)
    let aof = match aof::spawn_aof_writer("ignix.aof") {
        Ok(handle) => Some(handle),
        Err(e) => {
            log::warn!("AOF persistence disabled: {e:#}");
            None
        }
    };

    // Create the main storage shard with ID 0
    // Currently Ignix uses a single shard, but architecture supports multiple
    let shard = shard::Shard::new(0, aof);

    #[cfg(target_os = "linux")]
    if args.uring {
        if args.busy_poll_given {
            log::warn!("--busy-poll-us only applies to the default (mio) backend");
        }
        return net_uring::run_server(args.addr, shard, args.options);
    }

    if args.uring {
        log::warn!("io_uring backend is only available on Linux, falling back to mio/epoll");
    }

    log::info!(
        "busy-poll window: {} µs",
        args.options.busy_poll.as_micros()
    );

    // Start the main server event loop
    // This call blocks until the server is shut down
    net::run_server(args.addr, shard, args.options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(OsString::from))
    }

    fn run(args: &[&str]) -> Args {
        match parse(args) {
            Ok(Command::Run(args)) => args,
            other => panic!("{args:?} gave {other:?}"),
        }
    }

    #[test]
    fn defaults_listen_on_every_address_at_7379() {
        let args = run(&[]);
        assert_eq!(args.addr, "0.0.0.0:7379".parse().unwrap());
        assert!(!args.uring && !args.busy_poll_given);
        assert_eq!(args.options.requirepass, None);
        assert_eq!(args.options.busy_poll, net::DEFAULT_BUSY_POLL);
    }

    #[test]
    fn options_take_a_separate_or_an_inline_value() {
        let args = run(&[
            "--bind",
            "127.0.0.1",
            "--port=6400",
            "--requirepass",
            "s3cret=x",
            "--backend=uring",
            "--busy-poll-us",
            "0",
        ]);
        assert_eq!(args.addr, "127.0.0.1:6400".parse().unwrap());
        assert_eq!(args.options.requirepass.as_deref(), Some("s3cret=x"));
        assert!(args.uring && args.busy_poll_given);
        assert_eq!(args.options.busy_poll, Duration::ZERO);
        // The last one wins, and IPv6 works
        let args = run(&["--port", "1", "--port", "65535", "--bind=::1"]);
        assert_eq!(args.addr, "[::1]:65535".parse().unwrap());
        assert_eq!(
            run(&["--requirepass="]).options.requirepass.as_deref(),
            Some("")
        );
    }

    #[test]
    fn help_and_version() {
        for arg in ["-h", "--help"] {
            assert!(matches!(parse(&["--port", "1", arg]), Ok(Command::Help)));
        }
        for arg in ["-V", "--version"] {
            assert!(matches!(parse(&[arg]), Ok(Command::Version)));
        }
    }

    #[test]
    fn usage_errors_name_the_problem() {
        let error = |args: &[&str]| parse(args).unwrap_err();
        assert_eq!(error(&["--nope"]), "unknown option '--nope'");
        assert_eq!(error(&["--nope=1"]), "unknown option '--nope'");
        assert_eq!(error(&["7379"]), "unknown option '7379'");
        assert_eq!(error(&["--port"]), "option '--port' needs a value");
        for port in ["0", "65536", "-1", "x", ""] {
            assert_eq!(
                error(&["--port", port]),
                format!("invalid --port {port:?}: expected 1 to 65535")
            );
        }
        assert!(error(&["--bind", "localhost"]).starts_with("invalid --bind address"));
        assert!(error(&["--backend", "epoll"]).starts_with("invalid --backend"));
        assert!(error(&["--busy-poll-us=-5"]).starts_with("invalid --busy-poll-us"));
    }
}
