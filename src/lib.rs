// Core modules for Ignix key-value store
pub mod protocol; // RESP parser + encoders + Cmd enum
pub mod storage; // Dict + Value types for in-memory storage
pub mod aof; // AOF writer + emit helpers for persistence
pub mod shard; // Shard::exec (command execution logic)
pub mod net; // bind_reuseport + run_shard (server loop)

// Re-export all public items from modules for easier access
pub use protocol::*;
pub use storage::*;
pub use aof::*;
pub use shard::*;
pub use net::*;

// Default server address - Redis-compatible port 7379
pub const DEFAULT_ADDR: &str = "0.0.0.0:7379";