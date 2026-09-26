# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security
- **Remote crash on malformed requests**: a negative or oversized bulk length (e.g. `*1\r\n$-5\r\n`) made the parser overflow, and a huge element count (`*9223372036854775807\r\n`) made it try to allocate that many elements. Because the release profile uses `panic = "abort"`, a single packet from any client terminated the whole server. Lengths are now parsed with checked arithmetic, bulk strings are limited to 512 MiB and element counts to `INT_MAX` (as in Redis), and malformed input is answered with `ERR Protocol error: ...`.

### Added
- **Multi-key `DEL` and `EXISTS`, `PING [message]`**: `DEL` removes every given key and replies with the number removed, `EXISTS` counts every existing argument (repeated keys count each time), and `PING message` echoes the message as a bulk string, as in Redis.

- **Request API for servers**: `protocol::parse_requests` parses every complete request into `Request::Cmd` or `Request::Invalid(error_line)` and only fails on protocol errors, and `protocol::write_error` writes a RESP error reply (`-ERR ...`).

### Changed
- **`parse_many` no longer gets stuck on an invalid command**: the invalid request is consumed before the error is returned, so the next call continues with the following request. Protocol errors still leave the malformed bytes in the buffer.
- **Breaking: command enum shapes**: `Cmd::Ping` is now `Cmd::Ping(Option<Bytes>)`, `Cmd::Del(Bytes)` is `Cmd::Del(Vec<Bytes>)` and `Cmd::Exists(Bytes)` is `Cmd::Exists(Vec<Bytes>)`. `Cmd` and `Value` are `#[non_exhaustive]`, so future commands and value types are not breaking changes.
- **Redis-style argument validation**: a wrong number of arguments is rejected with `ERR wrong number of arguments for '<command>' command`, an unknown command with `ERR unknown command '<name>', with args beginning with: ...` (truncated like Redis), and `SET` options that are not implemented yet (`NX`, `XX`, `GET`, `EX`, `PX`, `EXAT`, `PXAT`, `KEEPTTL`) with `ERR SET option '<name>' is not supported`; any other extra `SET` token is `ERR syntax error`.
- **Stricter request framing**: length lines are parsed like Redis `string2ll` (no leading zeros, `+`, spaces or lines longer than 20 characters). Unlike Redis, a bulk payload must be followed by CRLF instead of skipping two bytes blindly, which stops a miscounted length from desynchronising the stream. Empty requests (`*0\r\n`, `*-1\r\n`) are ignored like in Redis instead of being an error.

### Fixed
- **Silently ignored arguments**: `DEL a b c` deleted only `a`, `EXISTS a b` checked only `a`, `GET a b` and `INCR a b` ignored the extra argument, and `SET k v NX` or `SET k v EX 10` set the key unconditionally and without expiry while replying `OK`.
- **Linux build**: the crate did not compile on Linux (and therefore not on docs.rs) because `src/net_uring.rs` used an `Ok(_)` pattern that resolved to `anyhow::Ok` (E0532) and borrowed the io_uring instance mutably twice (E0499).

### Migration Notes
- Build `Cmd::Ping(None)`, `Cmd::Del(vec![key])` and `Cmd::Exists(vec![key])` where the old unit/single-key variants were used, and add a wildcard arm to exhaustive matches on `Cmd` and `Value`.

## [0.3.2] - 2025-12-04

### Added
- **io_uring Backend (Linux)**: Implemented an experimental `io_uring` backend for Linux, enabling high-performance asynchronous I/O with reduced syscall overhead.
- **Backend Selection**: Added `--backend` command-line argument. Use `--backend=uring` to enable the new backend on Linux. Falls back to `mio` (epoll) on other platforms.

## [0.3.1] - 2025-12-04

### Changed
- **Zero-Copy Response Generation**: Refactored `Shard::exec` and network layer to write responses directly to the output buffer (`BytesMut`), eliminating intermediate `Vec<u8>` allocations and double-copying.
- **Protocol**: Added `write_*` helpers in `src/protocol.rs` for direct buffer writing.

### Performance
- **GET Latency**: Reduced `GET` latency to match `SET` operations (~1.85ms vs 1.66ms).
- **Throughput**: `GET` throughput increased to ~23k ops/sec, now comparable to Redis.
- **Large Payloads**: Verified competitive performance with 2MB payloads (754 ops/sec vs Redis 845 ops/sec).

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
