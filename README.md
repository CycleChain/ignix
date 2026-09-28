# 🔥 Ignix

**High-Performance Redis-Compatible Key-Value Store**

Ignix (from "Ignite" + "Index") is a Redis-protocol compatible, in-memory key-value store designed for modern multi-core systems. Built with Rust for performance and safety.

[![Rust](https://img.shields.io/badge/rust-1.80+-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)

## ✨ Features

- 🚀 **Multi-core**: one event loop per CPU core, sharing the listening port with `SO_REUSEPORT`
- 🔌 **Redis protocol (RESP2)**: works with `redis-cli` and Redis client libraries (redis-py, node-redis, ...) for the supported commands
- 🧵 **Async I/O**: `mio` (epoll/kqueue) by default, optional `io_uring` backend on Linux
- ⚡ **Busy-polling**: a worker keeps polling for 50 µs after its last event before it sleeps, so requests rarely wait for a thread to wake up (`--busy-poll-us`)
- 💾 **AOF**: every write is appended to `ignix.aof` by a background thread and synced at most once per second
- 🧠 **Concurrent storage**: 1024 hash-table shards with one read-write lock each
- 📊 **Benchmarks included**: criterion micro-benchmarks and a Redis comparison suite

## 🏗️ Architecture

- **Thread per core**: `SO_REUSEPORT` lets one worker thread per CPU core accept connections on the same port; the kernel spreads new connections across them.
- **Pluggable backend**: `mio` (epoll/kqueue) or `io_uring` (Linux only, single thread).
- **Independent event loops**: each thread owns its connections and runs commands inline; all threads share one concurrent dictionary.
- **Busy-polling**: waking a sleeping thread costs more than serving a small request, especially on virtual machines, so after its last event a worker polls without blocking for 50 µs before it sleeps. An idle server sleeps.
- **Allocation-free replies**: replies are written straight into the connection's output buffer.
- **RESP parsing**: requests are RESP arrays of bulk strings, parsed with the same length rules and limits as Redis (at most 512 MB per argument).
- **Concurrent storage**: the keyspace is split into 1024 hashbrown tables, each behind its own read-write lock; a key is hashed once (SipHash with random keys) to pick its shard and its slot. Canonical integers are stored as numbers, everything else byte for byte.
- **AOF persistence**: dedicated writer thread fed by a bounded channel (back-pressure); queued records are written in batches and synced at most once per second.

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

### Command-line Options

- `--backend=uring`: use the io_uring backend (Linux only; elsewhere Ignix falls back to mio).
- `--busy-poll-us=N`: how long a worker of the default (mio) backend keeps polling for new events after its last one before it sleeps, in microseconds; the default is 50 and `0` disables busy-polling. It trades CPU time for latency: an idle server uses no CPU either way, at 1,000 requests per second the server used about 10% of a CPU instead of 5% in our measurements, and under sustained load every busy worker uses a full core.

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

Measured on the unreleased code on a shared cloud VM: 4 vCPUs (Intel Xeon @ 2.10 GHz, one thread per core), 15 GB RAM, Linux 6.18, with servers and clients on the same machine. Redis 7.0.15 ran with `--appendonly yes --appendfsync everysec --save ""`, so both servers append every write to a file and sync it about once per second. Ignix used its defaults: one worker thread per core and a 50 µs busy-poll window; the "no busy-poll" column is `--busy-poll-us=0`. Redis executes commands on one thread. The servers ran one after another in two rounds (in alternating order, with a 3 s pause between each SET and GET run); cells show `round 1 / round 2`.

### redis-benchmark, one client thread

`redis-benchmark -t set,get -c 50 -n 500000 -d 64` (or `-d 1024`), and `-n 1000000 -P 16` (16 pipelined requests per connection); nothing pinned to CPUs, 4 Ignix workers:

| Test | Redis (req/s) | Ignix (req/s) | Ignix/Redis | Ignix, no busy-poll |
|------|---------------|---------------|-------------|---------------------|
| SET 64 B | 79,378 / 79,239 | 62,846 / 82,645 | 0.79x / 1.04x | 65,488 / 55,157 |
| GET 64 B | 72,359 / 76,254 | 67,458 / 88,842 | 0.93x / 1.17x | 67,222 / 55,928 |
| SET 1 KB | 81,090 / 72,296 | 75,850 / 67,705 | 0.94x / 0.94x | 58,316 / 61,312 |
| GET 1 KB | 74,427 / 76,092 | 77,316 / 78,309 | 1.04x / 1.03x | 59,446 / 65,198 |
| SET 64 B, pipelined | 517,063 / 573,723 | 905,797 / 760,456 | 1.75x / 1.33x | 746,826 / 790,514 |
| GET 64 B, pipelined | 853,971 / 934,579 | 1,186,240 / 1,142,857 | 1.39x / 1.22x | 777,605 / 897,666 |

With a single client thread on a 4-vCPU machine the client is the bottleneck, and a large part of each request's cost is waking the server thread that handles it. Busy-polling keeps Ignix's workers awake while requests keep coming, which puts it about level with Redis without pipelining and ahead of it with pipelining. These runs vary a lot because the busy-polling workers share the four CPUs with the client.

### redis-benchmark, server-bound

Both servers pinned to CPUs 0-1 (`taskset -c 0-1`, so Ignix runs 2 workers) and `redis-benchmark --threads 2 -c 50 -n 1000000 -d 64` on CPUs 2-3:

| Test | Redis (req/s) | Ignix (req/s) | Ignix/Redis | Ignix, no busy-poll |
|------|---------------|---------------|-------------|---------------------|
| SET 64 B | 97,523 / 108,061 | 121,080 / 190,404 | 1.24x / 1.76x | 124,891 / 137,874 |
| GET 64 B | 124,938 / 133,262 | 159,923 / 181,719 | 1.28x / 1.36x | 97,494 / 133,280 |

When the client is not the bottleneck, Ignix's two workers serve 1.2-1.8x as many requests as Redis's single thread on the same two CPUs, with half the median latency (0.13-0.17 ms against 0.27-0.39 ms).

### Python suite (`benchmarks/`)

`benchmarks/scripts/comprehensive_benchmark.py` and `real_world_benchmark.py`, with both servers running at the same time; every reply is checked and all runs had 0 errors. The client is Python with one thread per connection, so these numbers show the client's limit more than the servers'.

| Operation | Size | Conns | Redis (ops/s) | Ignix (ops/s) | Ignix/Redis |
|-----------|------|-------|---------------|---------------|-------------|
| SET | 64 B | 50 | 13,236 / 12,353 | 13,062 / 12,376 | 0.99x / 1.00x |
| GET | 64 B | 50 | 13,945 / 10,652 | 12,736 / 10,684 | 0.91x / 1.00x |
| SET | 1 KB | 50 | 13,199 / 10,173 | 10,334 / 12,142 | 0.78x / 1.19x |
| GET | 1 KB | 50 | 11,464 / 10,175 | 9,717 / 11,268 | 0.85x / 1.11x |
| SET | 32 KB | 20 | 8,138 / 10,733 | 8,910 / 7,730 | 1.09x / 0.72x |
| GET | 32 KB | 20 | 7,570 / 10,555 | 10,275 / 10,058 | 1.36x / 0.95x |
| SET | 256 KB | 20 | 751 / 516 | 1,066 / 1,830 | 1.42x / 3.55x |
| GET | 256 KB | 20 | 4,234 / 4,052 | 4,124 / 3,804 | 0.97x / 0.94x |
| SET | 2 MB | 10 | 39 / 40 | 106 / 67 | 2.69x / 1.66x |
| GET | 2 MB | 10 | 712 / 766 | 751 / 907 | 1.05x / 1.18x |

Real-world scenario (session store: 80% GET / 20% SET, 10,000 keys, Zipfian access, 1-2 KB values, 50 connections):

| Metric | Redis | Ignix | Ignix/Redis |
|--------|-------|-------|-------------|
| Throughput | 10,167 / 11,002 ops/s | 10,922 / 4,477 ops/s | 1.07x / 0.41x |
| Avg latency | 4.78 / 4.42 ms | 4.43 / 11.00 ms | 0.93x / 2.49x |
| p99 latency | 15.7 / 14.1 ms | 15.8 / 132.0 ms | 1.01x / 9.36x |

Ignix is ahead on large writes, where Redis also rewrites its AOF in the background. The second real-world round was much slower for Ignix, for GETs as well as SETs. It ran right after the large-value runs had appended about 6 GB to Ignix's AOF (12 GB in total, as it is never compacted), on a disk shared with Redis. Repeated on a fresh server right after a 5.5 GB burst of SETs on 20 connections, the scenario was 15% slower than without the burst and AOF syncs took up to 0.3 s, but the collapse did not reappear, so its exact cause is not known; the AOF path is the main suspect. Taking the fsync off the AOF writer's path and compacting the AOF are planned. The earlier tables for v0.3.1 could not be reproduced: the scripts that produced them did not check GET replies or count errors.

### 📊 Benchmark Your Own Workload

```bash
# Build, start Ignix and Redis (AOF, everysec) and run everything
bash benchmarks/run_benchmarks.sh

# With both servers already running
python3 benchmarks/run_all.py
python3 benchmarks/quick_benchmark.py

# Server throughput with the C client
redis-benchmark -p 7379 -t set,get -n 500000 -c 50 -q
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
- The AOF is write-only: it is not loaded on startup, so data does not survive a restart, and it is never compacted, so it grows with every write. In one of our benchmark sessions `ignix.aof` grew to 13.8 GB while Redis, which rewrites its AOF, used 2.3 GB.
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