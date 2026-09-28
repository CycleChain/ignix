// Core modules for Ignix key-value store
pub mod aof; // AOF writer + emit helpers for persistence
mod commands; // command table: names and arity
pub mod net; // bind_reuseport + run_shard (server loop)
pub mod net_uring;
pub mod protocol; // RESP parser + encoders + Cmd enum
pub mod shard; // Shard::exec (command execution logic)
pub mod storage; // Dict + Value types for in-memory storage

// Re-export all public items from modules for easier access
pub use aof::*;
pub use net::*;
pub use protocol::*;
pub use shard::*;
pub use storage::*;

// Default server address - Redis-compatible port 7379
pub const DEFAULT_ADDR: &str = "0.0.0.0:7379";
