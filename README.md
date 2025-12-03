# 🔥 Ignix

**High-Performance Redis-Compatible Key-Value Store**

Ignix (from "Ignite" + "Index") is a blazing-fast, Redis-protocol compatible key-value store designed for modern multi-core systems. Built with Rust for maximum performance and safety.

[![Rust](https://img.shields.io/badge/rust-1.90+-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)

## ✨ Features

- 🚀 **High Performance**: Rust + reactor/worker model for parallel execution
- 🔌 **Redis Protocol Compatible**: Drop-in replacement for Redis clients
- 🧵 **Async I/O (mio)**: Non-blocking reactor + `mio::Waker` response path
- 💾 **AOF Persistence**: Background writer with bounded backpressure
- 🧠 **Concurrent Storage**: `DashMap` (sharded locking) in hot path
- 📊 **Benchmarks Included**: Scripts and criterion benches

## 🏗️ Architecture

Ignix v0.3.1 architecture:

- **Multi-Reactor (Thread-per-Core)**: Uses `SO_REUSEPORT` to spawn N independent worker threads (one per CPU core).
- **Shared Nothing**: Each thread has its own event loop (`mio::Poll`) and handles connections independently.
- **Zero-Lock Networking**: No shared listener lock; kernel distributes incoming connections.
- **Zero-Copy Response**: Responses are written directly to the network buffer, avoiding intermediate allocations.
- **RESP Protocol**: Full Redis Serialization Protocol support with optimized SWAR parsing.
- **Concurrent Storage**: `DashMap<Bytes, Value>` (sharded locking) for high-concurrency data access.
- **AOF Persistence**: Dedicated thread, bounded channel, periodic fsync.

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

See the Performance section and `benchmark_results/benchmark_results.json`.

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

Benchmarks reflect Ignix v0.3.1. Full raw results are in `benchmarks/results/`.

### SET Throughput (ops/sec)

| Data | Conns | Redis | Ignix | Ratio (Ignix/Redis) |
|------|-------|-------|-------|----------------------|
| 64B  | 1     | 9,249 | 9,116 | 0.99x |
| 64B  | 10    | 17,628 | 22,360 | 1.27x |
| 64B  | 50    | 18,236 | 23,993 | 1.32x |
| 256B | 1     | 14,615 | 4,738 | 0.32x |
| 256B | 10    | 17,880 | 16,273 | 0.91x |
| 256B | 50    | 16,898 | 16,959 | 1.00x |
| 1KB  | 1     | 16,300 | 6,451 | 0.40x |
| 1KB  | 10    | 16,936 | 24,323 | 1.44x |
| 1KB  | 50    | 3,313 | 7,314 | 2.21x |
| 4KB  | 1     | 11,286 | 8,581 | 0.76x |
| 4KB  | 10    | 17,232 | 27,933 | 1.62x |
| 4KB  | 50    | 16,343 | 20,675 | 1.27x |

### GET Throughput (ops/sec)

| Data | Conns | Redis | Ignix | Ratio (Ignix/Redis) |
|------|-------|-------|-------|----------------------|
| 64B  | 50    | 35,235 | 42,495 | **1.21x** |
| 1KB  | 50    | 33,701 | 39,466 | **1.17x** |
| 32KB | 20    | 15,912 | 21,445 | **1.35x** |
| 256KB| 20    | 18,497 | 17,408 | 0.94x |
| 2MB  | 10    | 1,054 | 2,062 | **1.96x** |

> **Note**: v0.3.1 introduced Zero-Copy Response Generation, significantly boosting GET performance. Ignix now consistently outperforms or matches Redis across most payload sizes.

### Real-World Scenario (Session Store)

| Metric | Redis | Ignix | Ratio (Ignix/Redis) |
|--------|-------|-------|----------------------|
| Throughput | 3,201 ops/sec | 3,996 ops/sec | **1.25x** |
| Avg Latency | 13.56 ms | 10.38 ms | **0.76x** |

Notes:
- Values rounded from `benchmark_results/benchmark_results.json`.
- All runs showed 0 errors, 100% success.

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

Enable debug logging: `RUST_LOG=debug cargo run --release`
Monitor AOF: `tail -f ignix.aof`

## 🚧 Roadmap (Short)

- More Redis commands (HASH/LIST/SET)
- RDB snapshots, metrics/monitoring
- Clustering and replication

## 🐛 Known Limitations

- Limited command set vs Redis (expanding)
- No clustering or replication yet
- RDB snapshots not yet available

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