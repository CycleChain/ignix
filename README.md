# 🔥 Ignix

**High-Performance Redis-Compatible Key-Value Store**

Ignix (from "Ignite" + "Index") is a Redis-protocol compatible, in-memory key-value store designed for modern multi-core systems. Built with Rust for performance and safety.

[![Rust](https://img.shields.io/badge/rust-1.80+-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)

## ✨ Features

- 🚀 **Multi-core**: one event loop per CPU core, sharing the listening port with `SO_REUSEPORT`
- 🔌 **Redis protocol (RESP2)**: works with `redis-cli` and Redis client libraries (redis-py, node-redis, ...) for the supported commands
- 🧵 **Async I/O**: `mio` (epoll/kqueue) by default, optional `io_uring` backend on Linux
- 💾 **AOF**: every write is appended to `ignix.aof` by a background thread and synced at most once per second
- 🧠 **Concurrent storage**: `DashMap` (sharded locking) in the hot path
- 📊 **Benchmarks included**: criterion micro-benchmarks and a Redis comparison suite

## 🏗️ Architecture

- **Thread per core**: `SO_REUSEPORT` lets one worker thread per CPU core accept connections on the same port; the kernel spreads new connections across them.
- **Pluggable backend**: `mio` (epoll/kqueue) or `io_uring` (Linux only, single thread).
- **Independent event loops**: each thread owns its connections and runs commands inline; all threads share one concurrent dictionary.
- **Allocation-free replies**: replies are written straight into the connection's output buffer.
- **RESP parsing**: requests are RESP arrays of bulk strings, parsed with the same length rules and limits as Redis (at most 512 MB per argument).
- **Concurrent storage**: `DashMap<Bytes, Value>`; canonical integers are stored as numbers, everything else byte for byte.
- **AOF persistence**: dedicated writer thread, bounded channel for back-pressure, fsync at most once per second.

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
# Or enable io_uring backend (Linux only)
cargo run --release -- --backend=uring
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

| Command | Description | Example |
|---------|-------------|---------|
| `PING [message]` | Test connectivity | `PING` → `+PONG`, `PING hi` → `$2\r\nhi` |
| `SET key value` | Set a value | `SET key value` → `+OK` |
| `GET key` | Get a value | `GET key` → `$5\r\nvalue` |
| `DEL key [key ...]` | Delete keys, reply with the number removed | `DEL a b` → `:2` |
| `EXISTS key [key ...]` | Count how many of the keys exist | `EXISTS a b` → `:1` |
| `INCR key` | Increment an integer | `INCR counter` → `:1` |
| `INCRBY key increment` | Add to an integer | `INCRBY counter 5` → `:6` |
| `DECR key` | Decrement an integer | `DECR counter` → `:5` |
| `DECRBY key decrement` | Subtract from an integer | `DECRBY counter 2` → `:3` |
| `RENAME key newkey` | Rename a key | `RENAME old new` → `+OK` |
| `MGET key [key ...]` | Get multiple values | `MGET k1 k2` → `*2\r\n...` |
| `MSET key value [key value ...]` | Set multiple values | `MSET k1 v1 k2 v2` → `+OK` |

Replies and error messages match Redis 7, for example `-ERR wrong number of arguments for 'get' command`, `-ERR unknown command 'FOO', with args beginning with: ...` and `-ERR value is not an integer or out of range`. After an invalid command the connection keeps working; after malformed RESP the server replies `-ERR Protocol error: ...` and closes the connection, as Redis does.

`SET` options (`EX`, `PX`, `NX`, `XX`, `GET`, `EXAT`, `PXAT`, `KEEPTTL`) are not implemented yet; they are rejected with `-ERR SET option '<NAME>' is not supported` instead of being ignored.

## 🔧 Configuration

### Environment Variables

- `RUST_LOG`: log level for diagnostics (`error`, `warn`, `info`, `debug`); the default is `info`.

### AOF Persistence

Ignix appends every write (`SET`, `MSET`, `DEL`, `RENAME`, `INCR`, `INCRBY`, `DECR`, `DECRBY`) to `ignix.aof` in its working directory, in RESP format, and syncs it to disk at most one second after a write. The file is not loaded on startup yet, so data does not survive a restart. If the file cannot be opened, Ignix logs a warning and runs without persistence.

## 🧪 Testing

```bash
cargo test
```

Tests that need a running server (`tests/network.rs`, `tests/large_payloads.rs`) are ignored by default. Start a server and include them:

```bash
cargo run --release              # in another terminal
cargo test -- --include-ignored
```

Lints: `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`.

### Micro-benchmarks

```bash
cargo bench --bench exec    # Shard::exec
cargo bench --bench resp    # RESP parser and reply writers
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

Ignix speaks RESP2. Clients that default to RESP3, such as redis-py 8, must be created with RESP2:

```python
import redis

# Connect to Ignix
r = redis.Redis(host='localhost', port=7379, protocol=2, decode_responses=True)

# Use like Redis
r.set('hello', 'world')
print(r.get('hello'))  # Output: world
```

See [examples/](examples/) for complete Rust, Python and Node.js clients.

## 📊 Performance

Measured on the unreleased code on a shared cloud VM: 4 vCPUs (Intel Xeon @ 2.10 GHz, one thread per core), 15 GB RAM, Linux 6.18, with servers and clients on the same machine. Redis 7.0.15 ran with `--appendonly yes --appendfsync everysec --save ""`, so both servers append every write to a file and sync it about once per second. Ignix used its default one worker thread per core (4); Redis executes commands on one thread. Every configuration ran on both servers back to back, in two rounds; cells show `round 1 / round 2`.

### redis-benchmark (C client)

`redis-benchmark -t set,get -c 50` with `-n 200000`, or `-n 1000000 -P 16` (16 pipelined requests per connection):

| Test | Redis (req/s) | Ignix (req/s) | Ignix/Redis |
|------|---------------|---------------|-------------|
| SET 64 B | 110,254 / 107,009 | 73,260 / 67,408 | 0.66x / 0.63x |
| GET 64 B | 97,276 / 103,040 | 83,612 / 80,321 | 0.86x / 0.78x |
| SET 1 KB | 98,184 / 99,602 | 71,048 / 74,627 | 0.72x / 0.75x |
| GET 1 KB | 88,456 / 100,100 | 78,094 / 79,872 | 0.88x / 0.80x |
| SET 64 B, pipelined | 790,514 / 846,740 | 1,262,626 / 1,240,695 | 1.60x / 1.47x |
| GET 64 B, pipelined | 1,422,475 / 1,577,287 | 1,351,351 / 1,129,944 | 0.95x / 0.72x |

Without pipelining every request is one round trip and Redis is faster. Turning the Ignix AOF off only raised non-pipelined SET to 74,878 / 77,279 req/s, so the append-only file is not the main cost. With pipelining Ignix spreads the connections over its threads and sustains about 1.5x Redis for SET.

### Python suite (`benchmarks/`)

`benchmarks/run_all.py`, where every reply is checked; all runs had 0 errors. The client is Python with one thread per connection, so for small values these numbers show the client's limit rather than the servers'.

| Operation | Size | Conns | Redis (ops/s) | Ignix (ops/s) | Ignix/Redis |
|-----------|------|-------|---------------|---------------|-------------|
| SET | 64 B | 50 | 15,102 / 15,126 | 14,815 / 15,215 | 0.98x / 1.01x |
| GET | 64 B | 50 | 13,803 / 14,228 | 14,109 / 14,289 | 1.02x / 1.00x |
| SET | 1 KB | 50 | 16,675 / 14,716 | 15,328 / 13,618 | 0.92x / 0.93x |
| GET | 1 KB | 50 | 14,526 / 14,674 | 14,559 / 13,751 | 1.00x / 0.94x |
| SET | 32 KB | 20 | 7,897 / 10,646 | 12,061 / 14,243 | 1.53x / 1.34x |
| GET | 32 KB | 20 | 13,032 / 11,945 | 12,757 / 11,730 | 0.98x / 0.98x |
| SET | 256 KB | 20 | 1,624 / 2,862 | 2,084 / 6,708 | 1.28x / 2.34x |
| GET | 256 KB | 20 | 4,990 / 4,850 | 4,216 / 4,600 | 0.84x / 0.95x |
| SET | 2 MB | 10 | 95 / 90 | 190 / 299 | 1.99x / 3.32x |
| GET | 2 MB | 10 | 755 / 970 | 1,132 / 1,176 | 1.50x / 1.21x |

Real-world scenario (session store: 80% GET / 20% SET, 10,000 keys, Zipfian access, 1-2 KB values, 50 connections):

| Metric | Redis | Ignix | Ignix/Redis |
|--------|-------|-------|-------------|
| Throughput | 13,954 / 14,350 ops/s | 13,876 / 13,802 ops/s | 0.99x / 0.96x |
| Avg latency | 3.50 / 3.40 ms | 3.52 / 3.55 ms | 1.00x / 1.04x |

Ignix is ahead on large writes, where Redis also rewrites its AOF in the background (Ignix never compacts its AOF; see Known Limitations). The earlier tables for v0.3.1 could not be reproduced: the scripts that produced them did not check GET replies or count errors.

### 📊 Benchmark Your Own Workload

```bash
# Build, start Ignix and Redis (AOF, everysec) and run everything
bash benchmarks/run_benchmarks.sh

# With both servers already running
python3 benchmarks/run_all.py
python3 benchmarks/quick_benchmark.py

# Server throughput with the C client
redis-benchmark -p 7379 -t set,get -n 200000 -c 50 -q
```

See [benchmarks/BENCHMARK_README.md](benchmarks/BENCHMARK_README.md) for every option and for how to compare fairly.

## 🏗️ Development

### Project Structure

```
src/
├── bin/ignix.rs        # Server binary
├── lib.rs              # Library exports
├── protocol.rs         # RESP parser, command validation, reply writers
├── storage.rs          # In-memory storage (Dict)
├── shard.rs            # Command execution logic
├── net.rs              # mio networking and event loop
├── net_uring.rs        # io_uring backend (Linux)
└── aof.rs              # AOF persistence

examples/               # Rust, Python and Node.js clients

tests/
├── basic.rs            # Basic command flow
├── commands.rs         # Command semantics, checked against Redis replies
├── aof.rs              # AOF encoding and logging
├── protocol_framing.rs # RESP framing and limits
├── protocol_api.rs     # Request parsing and reply APIs
├── resp.rs             # Protocol parsing
├── network.rs          # Connection behaviour (needs a running server)
└── large_payloads.rs   # 100 KB - 10 MB values (needs a running server)

benches/
├── exec.rs             # Command execution benchmarks
└── resp.rs             # Parser and reply writer benchmarks

benchmarks/             # Redis comparison suite (Python), see BENCHMARK_README.md
```

### Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Make your changes
4. Add tests for new functionality
5. Run tests (`cargo test`, plus `cargo test -- --include-ignored` with a server running)
6. Run lints (`cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`)
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

- More Redis commands (HASH/LIST/SET) and key expiry
- Loading the AOF on startup, RDB snapshots, metrics/monitoring
- Clustering and replication

## 🐛 Known Limitations

- Limited command set compared with Redis (no `KEYS`, `INFO`, `CLIENT`, `CONFIG`, `SELECT`, `QUIT`, ...).
- RESP2 only: no RESP3 or `HELLO`, and no inline commands (plain text lines such as `PING` typed into telnet).
- No key expiry: `SET` options and `EXPIRE` are not implemented.
- The AOF is write-only: it is not loaded on startup, so data does not survive a restart, and it is never compacted, so it grows with every write. The server used for the Performance section ended with a 13.8 GB `ignix.aof`; Redis, which rewrites its AOF, used 2.3 GB.
- Multi-key commands (`MSET`, `DEL`, `RENAME`) are not atomic with respect to other connections.
- No authentication, and the server listens on the fixed address `0.0.0.0:7379`; do not expose it to untrusted networks.
- The io_uring backend runs on a single thread.
- No clustering or replication.

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