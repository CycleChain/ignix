/*!
 * Ignix Server Main Entry Point
 *
 * This is the main executable that starts the Ignix key-value server.
 * It initializes logging, creates the storage shard, optionally enables
 * AOF persistence, and starts the main server event loop.
 */

use anyhow::Result;
use ignix::*;
use std::net::ToSocketAddrs;
use std::time::Duration;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Main function - entry point for Ignix server
///
/// Initializes the server components and starts the main event loop:
/// 1. Initialize logging system
/// 2. Parse server address
/// 3. Create AOF writer (if possible)
/// 4. Create storage shard
/// 5. Start server event loop
fn main() -> Result<()> {
    // Initialize logging - respects RUST_LOG environment variable and shows
    // info and above by default. Example: RUST_LOG=debug cargo run --release
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Parse arguments
    let args: Vec<String> = std::env::args().collect();
    let use_uring = args.iter().any(|a| a == "--backend=uring");

    // --busy-poll-us=N: busy-poll window of the mio backend (0 disables it)
    let mut options = net::ServerOptions::default();
    let mut busy_poll_given = false;
    for arg in &args[1..] {
        if let Some(value) = arg.strip_prefix("--busy-poll-us=") {
            let micros: u64 = value
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid --busy-poll-us value: {value:?}"))?;
            options.busy_poll = Duration::from_micros(micros);
            busy_poll_given = true;
        }
    }

    // Parse the default server address (0.0.0.0:7379)
    let addr = DEFAULT_ADDR.to_socket_addrs()?.next().unwrap();

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

    // Print startup message
    println!("ignix running on {}", addr);

    #[cfg(target_os = "linux")]
    if use_uring {
        if busy_poll_given {
            log::warn!("--busy-poll-us only applies to the default (mio) backend");
        }
        return net_uring::run_shard(0, addr, shard);
    }

    if use_uring {
        log::warn!("io_uring backend is only available on Linux, falling back to mio/epoll");
    }

    log::info!("busy-poll window: {} µs", options.busy_poll.as_micros());

    // Start the main server event loop
    // This call blocks until the server is shut down
    net::run_server(addr, shard, options)
}
