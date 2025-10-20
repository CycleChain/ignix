# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2025-11-20

### Changed
- Migrated from standard `std::collections::HashMap` to SwissTable implementation via `hashbrown::HashMap`
- Updated core storage layer (`src/storage.rs`) to use hashbrown for better performance
- Updated network layer (`src/net.rs`) client connection storage to use SwissTable
- Added `hashbrown = "0.14"` dependency for SwissTable support

### Performance
- Improved hash table performance with SwissTable (hashbrown) implementation
- Better memory efficiency and faster lookups compared to standard HashMap
- Maintained full API compatibility - no breaking changes

## [0.1.0] - 2025-10-22

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
