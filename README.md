# 🔥 Ignix

**High-Performance Redis-Compatible Key-Value Store**

Ignix (from "Ignite" + "Index") is a blazing-fast, Redis-protocol compatible key-value store designed for modern multi-core systems. Built with Rust for maximum performance and safety.

[![Rust](https://img.shields.io/badge/rust-1.90+-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)

## ✨ Features

- 🚀 **High Performance**: Built with Rust for maximum speed and safety
- 🔌 **Redis Protocol Compatible**: Drop-in replacement for Redis clients
- 🧵 **Async I/O**: Non-blocking networking with mio for high concurrency
- 💾 **AOF Persistence**: Append-only file for data durability
- 🎯 **Zero Dependencies**: Minimal external dependencies for security
- 📊 **Built-in Benchmarks**: Performance testing included

## 🏗️ Architecture

Ignix uses a simple but efficient architecture:

- **RESP Protocol**: Full Redis Serialization Protocol support
- **Event-Driven Networking**: mio-based async I/O for handling thousands of connections
- **In-Memory Storage**: SwissTable-based hash map storage for optimal performance
- **AOF Persistence**: Optional append-only file logging for durability

## 🚀 Quick Start

### Prerequisites

- Rust 1.80+ (recommended: latest stable)
- Cargo package manager

### Installation

```bash
git clone https://github.com/CycleChain/ignix.git
cd ignix
cargo build --release
```

### Running the Server

```bash
cargo run --release
```

The server will start on `0.0.0.0:7379` by default.

### Testing with Client Example

```bash
# In another terminal
cargo run --example client
```

Expected output:
```
+OK
$5
world
```

## 📡 Supported Commands

Ignix supports the following Redis commands:

| Command | Description | Example |
|---------|-------------|---------|
| `PING` | Test connectivity | `PING` → `+PONG` |
| `SET` | Set key-value pair | `SET key value` → `+OK` |
| `GET` | Get value by key | `GET key` → `$5\r\nvalue` |
| `DEL` | Delete key | `DEL key` → `:1` |
| `EXISTS` | Check if key exists | `EXISTS key` → `:1` |
| `INCR` | Increment integer value | `INCR counter` → `:1` |
| `RENAME` | Rename a key | `RENAME old new` → `+OK` |
| `MGET` | Get multiple values | `MGET key1 key2` → `*2\r\n...` |
| `MSET` | Set multiple key-value pairs | `MSET k1 v1 k2 v2` → `+OK` |

## 🔧 Configuration

### Environment Variables

- `RUST_LOG`: Set logging level (e.g., `debug`, `info`, `warn`, `error`)

### AOF Persistence

Ignix automatically creates an `ignix.aof` file for persistence. Data is written to AOF and flushed every second for durability.

## 🧪 Testing

### Run Unit Tests

```bash
cargo test
```

### Run Benchmarks

```bash
# Execute benchmark
cargo bench --bench exec

# RESP parsing benchmark  
cargo bench --bench resp
```

### Example Benchmark Results

```
exec/set_get            time:   [396.62 µs 403.23 µs 413.05 µs]
resp/parse_many_1k      time:   [296.51 µs 298.00 µs 299.44 µs]
```

## 🔌 Client Usage

### Using Redis CLI

```bash
redis-cli -h 127.0.0.1 -p 7379
127.0.0.1:7379> PING
PONG
127.0.0.1:7379> SET hello world
OK
127.0.0.1:7379> GET hello
"world"
```

### Using Any Redis Client Library

Ignix is compatible with any Redis client library. Here's a Python example:

```python
import redis

# Connect to Ignix
r = redis.Redis(host='localhost', port=7379, decode_responses=True)

# Use like Redis
r.set('hello', 'world')
print(r.get('hello'))  # Output: world
```

## 📊 Performance

> Benchmarks reflect Ignix v0.2.0 (reactor/worker split + DashMap). Full raw results are in `benchmark_results/benchmark_results.json`.

### SET Throughput (ops/sec)

| Data | Conns | Redis | Ignix | Ratio (Ignix/Redis) |
|------|-------|-------|-------|----------------------|
| 64B  | 1     | 9,249 | 9,116 | 0.99x |
| 64B  | 10    | 17,628 | 21,545 | 1.22x |
| 64B  | 50    | 18,236 | 15,515 | 0.85x |
| 256B | 1     | 14,615 | 12,032 | 0.82x |
| 256B | 10    | 17,880 | 28,032 | 1.57x |
| 256B | 50    | 16,898 | 19,306 | 1.14x |
| 1KB  | 1     | 16,300 | 11,083 | 0.68x |
| 1KB  | 10    | 16,936 | 20,644 | 1.22x |
| 1KB  | 50    | 17,155 | 20,663 | 1.20x |
| 4KB  | 1     | 11,286 | 8,581 | 0.76x |
| 4KB  | 10    | 17,232 | 28,136 | 1.63x |
| 4KB  | 50    | 16,343 | 17,131 | 1.05x |

### GET Throughput (ops/sec)

| Data | Conns | Redis | Ignix | Ratio (Ignix/Redis) |
|------|-------|-------|-------|----------------------|
| 64B  | 1     | 19,612 | 11,129 | 0.57x |
| 64B  | 10    | 16,780 | 28,873 | 1.72x |
| 64B  | 50    | 14,948 | 19,609 | 1.31x |
| 256B | 1     | 20,035 | 11,961 | 0.60x |
| 256B | 10    | 15,164 | 26,699 | 1.76x |
| 256B | 50    | 15,619 | 18,508 | 1.18x |
| 1KB  | 1     | 17,525 | 10,436 | 0.60x |
| 1KB  | 10    | 11,930 | 23,184 | 1.94x |
| 1KB  | 50    | 8,138 | 15,321 | 1.88x |
| 4KB  | 1     | 16,600 | 12,159 | 0.73x |
| 4KB  | 10    | 7,532 | 13,937 | 1.85x |
| 4KB  | 50    | 13,035 | 16,354 | 1.25x |

Notes:
- Values are rounded from `benchmark_results/benchmark_results.json`.
- All runs showed 0 errors and 100% success rate in the dataset provided.

### 📊 Benchmark Your Own Workload

Run comprehensive benchmarks with our included tools:

```bash
# Quick comparison
python3 quick_benchmark.py

# Detailed analysis with charts
python3 benchmark_redis_vs_ignix.py

# Custom test scenarios
python3 benchmark_redis_vs_ignix.py --data-sizes 64 256 1024 --connections 1 10 25
```

**Architecture Benefits:**
- **Sub-millisecond latency** for most operations
- **High throughput** with async I/O
- **Memory efficient** with zero-copy operations where possible
- **Minimal allocations** in hot paths

## 🏗️ Development

> **🚧 Early Development Stage**: Ignix is actively under development. APIs may change, and new features are being added regularly. We welcome contributions and feedback!

### Project Structure

```
src/
├── bin/ignix.rs        # Server binary
├── lib.rs              # Library exports
├── protocol.rs         # RESP protocol parser/encoder
├── storage.rs          # In-memory storage (Dict)
├── shard.rs           # Command execution logic  
├── net.rs             # Networking and event loop
└── aof.rs             # AOF persistence

examples/
└── client.rs          # Example client

tests/
├── basic.rs           # Basic functionality tests
└── resp.rs            # Protocol parsing tests

benches/
├── exec.rs            # Command execution benchmarks
└── resp.rs            # Protocol parsing benchmarks
```

### Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Make your changes
4. Add tests for new functionality
5. Run tests (`cargo test`)
6. Run benchmarks (`cargo bench`)
7. Commit your changes (`git commit -m 'Add amazing feature'`)
8. Push to the branch (`git push origin feature/amazing-feature`)
9. Open a Pull Request

### Code Style

- Follow Rust standard formatting (`cargo fmt`)
- Run Clippy lints (`cargo clippy`)
- Maintain test coverage for new features

## 🔍 Debugging

### Enable Debug Logging

```bash
RUST_LOG=debug cargo run --release
```

### Monitor AOF File

```bash
tail -f ignix.aof
```

## 🚧 Roadmap

**Current Development Phase**: Core optimization and stability

### 🎯 Short Term (Next Release)
- [ ] **Performance optimization** for large data operations
- [ ] **Memory management** improvements
- [ ] **Connection pooling** enhancements
- [ ] **Comprehensive benchmarking** suite expansion

### 🚀 Medium Term
- [ ] **More Redis commands** (HASH, LIST, SET operations)
- [ ] **Multi-threading** support for better concurrency
- [ ] **RDB snapshots** for faster restarts
- [ ] **Metrics and monitoring** endpoints

### 🌟 Long Term Vision
- [ ] **Clustering support** for horizontal scaling
- [ ] **Replication** for high availability
- [ ] **Lua scripting support** for complex operations
- [ ] **Advanced data structures** and algorithms
- [ ] **Plugin architecture** for extensibility

> **📈 Performance Goals**: Our primary focus is achieving consistently high performance across all data sizes while maintaining the simplicity and reliability that makes Ignix special.

## 🐛 Known Limitations

> **Development Status**: These limitations are actively being addressed as part of our development roadmap.

### Current Limitations
- **Single-threaded execution** (one shard) - *Multi-threading planned*
- **Limited command set** compared to full Redis - *Expanding gradually*
- **Large data performance** can be slower than Redis - *Optimization in progress*
- **No clustering or replication** yet - *Future releases*
- **AOF-only persistence** (no RDB snapshots) - *RDB support planned*

### Performance Notes
- **Excellent for small data** (64 bytes): Up to 4x faster SET operations than Redis
- **Competitive for medium data** (256 bytes - 1KB): Similar to Redis performance  
- **Room for improvement** on large payloads (4KB+): Redis shows maturity in large data handling

These characteristics make Ignix ideal for:
- ✅ **Caching layers** with small objects
- ✅ **Session storage** with quick access patterns
- ✅ **Real-time applications** requiring low latency
- ✅ **Microservices** with high-frequency, small data operations

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- [Redis](https://redis.io/) for the protocol specification
- [mio](https://github.com/tokio-rs/mio) for async I/O
- The Rust community for excellent tooling and libraries

## 📞 Support

- **Issues**: [GitHub Issues](https://github.com/CycleChain/ignix/issues)
- **Discussions**: [GitHub Discussions](https://github.com/CycleChain/ignix/discussions)

---

**Built with ❤️ and 🦀 by the [CycleChain.io](https://cyclechain.io) team**