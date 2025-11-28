/*!
 * Ignix Server Main Entry Point
 * 
 * This is the main executable that starts the Ignix key-value server.
 * It initializes logging, creates the storage shard, optionally enables
 * AOF persistence, and starts the main server event loop.
 */

use anyhow::*;
use ignix::*;
use std::net::ToSocketAddrs;

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
    // Initialize logging - respects RUST_LOG environment variable
    // Example: RUST_LOG=debug cargo run --release
    env_logger::init();
    
    // Parse the default server address (0.0.0.0:7379)
    let addr = DEFAULT_ADDR.to_socket_addrs()?.next().unwrap();
    
    // Try to create AOF writer for persistence
    // If this fails, server will run without persistence (in-memory only)
    let aof = aof::spawn_aof_writer("ignix.aof").ok();
    
    // Create the main storage shard with ID 0
    // Currently Ignix uses a single shard, but architecture supports multiple
    let shard = shard::Shard::new(0, aof);

    // Print startup message
    println!("ignix running on {}", addr);
    
    // Start the main server event loop
    // This call blocks until the server is shut down
    net::run_shard(0, addr, shard)
}