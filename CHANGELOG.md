# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2025-11-29

### Changed
- **Architecture**: Shifted to a **Multi-Reactor (Thread-per-Core)** architecture using `SO_REUSEPORT`. Each thread now runs its own event loop and handles connections independently, eliminating the worker pool bottleneck.
- **Networking**: Implemented `bind_reuseport` in `src/net.rs` using `socket2` to allow multiple threads to bind to the same port.
- **Protocol**: Optimized `read_decimal_line` in `src/protocol.rs` using **SWAR (SIMD Within A Register)** techniques for faster integer parsing.
- **I/O**: Implemented response batching in workers (before removal) and non-blocking I/O fixes for high concurrency.
- **Data Structures**: Migrated `Cmd` and `Value` to use `bytes::Bytes` for zero-copy string handling.
- **Build**: Optimized `Cargo.toml` profile (LTO, codegen-units=1, strip=true, panic=abort) and switched to `mimalloc` allocator.

### Added
- **Large Payload Support**: Verified support for 100KB, 1MB, and 10MB payloads with new tests.
- **Benchmarks**: Added comprehensive and real-world benchmark scripts (`benchmarks/`) with graphical reporting.

### Performance
- **Real-World**: Ignix now outperforms Redis by **~25%** in mixed read/write session store scenarios (3,996 vs 3,201 ops/sec).
- **Write Throughput**: Achieved **2.2x** higher throughput than Redis for 1KB SET operations (7,314 vs 3,313 ops/sec).
- **Concurrency**: Significantly improved scaling with concurrent connections due to lock-free/sharded architecture.

## [0.2.0] - 2025-11-03

### Changed
- Networking (`src/net.rs`): Decoupled reactor from command execution via bounded task/response channels and integrated `mio::Waker` to wake the reactor when worker responses are ready. Reactor no longer blocks on storage or disk I/O.
- Storage (`src/storage.rs`): Replaced single-threaded `HashMap` with concurrent `DashMap<Vec<u8>, Value>` (sharded locking) for improved parallel writes/reads. Introduced atomic `incr` using `entry` API.
- Shard (`src/shard.rs`): Made `Shard::exec` take `&self` to enable invocation from multiple worker threads; adapted to new `Dict` API.
- AOF (`src/aof.rs`): Switched to a bounded channel for backpressure; on shutdown performs a final flush and sync for graceful exit.
- Benches/Tests: Updated to `&self` for `Shard::exec`.
- Cargo: Added `dashmap` and `rustc-hash` dependencies; version bumped to `0.2.0`.

### Performance
- Eliminated reactor thread stalls by offloading command execution to a worker pool and using a waker-based response path.
- Reduced lock contention by moving to `DashMap` (sharded concurrency) in the hot path.
- Added bounded channels to prevent unbounded memory growth under load and to enforce backpressure.

### Migration Notes
- `Shard::exec` now takes `&self` instead of `&mut self`. Most callers do not require code changes beyond removing `mut`.

## [0.1.1] - 2025-10-20

### Changed
- Migrated from standard `std::collections::HashMap` to SwissTable implementation via `hashbrown::HashMap`
- Updated core storage layer (`src/storage.rs`) to use hashbrown for better performance
- Updated network layer (`src/net.rs`) client connection storage to use SwissTable
- Added `hashbrown = "0.14"` dependency for SwissTable support

### Performance
- Improved hash table performance with SwissTable (hashbrown) implementation
- Better memory efficiency and faster lookups compared to standard HashMap
- Maintained full API compatibility - no breaking changes

## [0.1.0] - 2025-09-22

### Added
- Initial release of Ignix Redis-compatible key-value store
- Core Redis protocol (RESP) support with PING, SET, GET, DEL, EXISTS, RENAME commands
- High-performance in-memory storage using AHash for optimized hashing
- Async I/O networking layer built with mio for high concurrency
- AOF (Append-Only File) persistence support
- Built-in benchmarking suite for performance testing
- Example clients in Rust, Python, and Node.js
- Comprehensive test suite covering basic operations and protocol parsing
- MIT license and complete documentation

### Features
- Drop-in Redis compatibility for existing clients
- Non-blocking event-driven architecture
- Memory-efficient storage with zero-copy operations where possible
- High throughput for small to medium-sized data operations
- Cross-platform support (Linux, macOS, Windows)
