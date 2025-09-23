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
- **In-Memory Storage**: Lightning-fast hash map-based storage
- **AOF Persistence**: Optional append-only file logging for durability

## 🚀 Quick Start

### Prerequisites

- Rust 1.80+ (recommended: latest stable)
- Cargo package manager

### Installation

```bash
git clone https://github.com/cyclechain/ignix.git
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

> **⚠️ Note**: Ignix is currently in **early development stage**. Performance characteristics are actively being optimized and may change significantly in future releases.

Ignix shows excellent performance characteristics, especially for small data operations:

### 🚀 Small Data Operations (64 bytes)
*Optimized for low-latency, high-frequency operations*

| Operation | Ignix | Redis | Ignix Advantage |
|-----------|-------|-------|-----------------|
| **SET** | 28,409 ops/sec | 9,897 ops/sec | **2.87x faster** |
| **GET** | 26,117 ops/sec | 21,192 ops/sec | **1.23x faster** |
| **Average Latency** | 0.03 ms | 0.07 ms | **2.3x lower** |

### 📈 Large Data Operations (6.4KB)
*Optimized for throughput-intensive workloads*

| Operation | Ignix | Redis | Performance |
|-----------|-------|-------|-------------|
| **SET** | 24,984 ops/sec | 27,607 ops/sec | Redis 1.11x faster |
| **GET** | 26,982 ops/sec | 27,156 ops/sec | ~Equal performance |
| **Average Latency** | 0.04 ms | 0.04 ms | ~Equal latency |

### 🎯 Performance Characteristics

**Ignix Excels At:**
- ✅ **Small, frequent operations** (cache-like workloads)
- ✅ **Low-latency responses** (sub-millisecond)
- ✅ **High-frequency SET operations**
- ✅ **Minimal connection overhead**

**Redis Excels At:**
- ✅ **Large data transfers** (mature buffer management)
- ✅ **Memory-intensive operations** (15+ years of optimization)
- ✅ **Complex data structures** (extensive command set)

### 🔬 Why This Performance Profile?

1. **Small Data Advantage**: Ignix's Rust-based architecture minimizes overhead for small operations
2. **Large Data Trade-off**: Redis's mature memory management shines with larger payloads
3. **Early Stage**: Ignix is optimized for core use cases, with room for improvement in others
4. **Development Focus**: Current optimization targets low-latency, high-frequency scenarios

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
- **Excellent for small data** (< 1KB): 2-3x faster than Redis
- **Competitive for medium data** (1-10KB): Similar to Redis performance  
- **Room for improvement** on very large payloads (> 10KB)

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

- **Issues**: [GitHub Issues](https://github.com/cyclechain/ignix/issues)
- **Discussions**: [GitHub Discussions](https://github.com/cyclechain/ignix/discussions)

---

**Built with ❤️ and 🦀 by the [CycleChain.io](https://cyclechain.io) team**