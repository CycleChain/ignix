# 🔥 Ignix

**High-Performance Redis-Compatible Key-Value Store**

Ignix (from "Ignite" + "Index") is a Redis-protocol compatible, in-memory key-value store designed for modern multi-core systems. Built with Rust for performance and safety.

[![Rust](https://img.shields.io/badge/rust-1.80+-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)

## ✨ Features

- 🚀 **Multi-core**: one event loop per CPU core, sharing the listening port with `SO_REUSEPORT`
- 🔌 **Redis protocol (RESP2 and RESP3)**: works with `redis-cli` and Redis client libraries (redis-py, node-redis, ...) for the supported commands
- 🧵 **Async I/O**: `mio` (epoll/kqueue) by default, optional `io_uring` backend on Linux
- ⚡ **Busy-polling**: a worker keeps polling for 50 µs after its last event before it sleeps, so requests rarely wait for a thread to wake up (`--busy-poll-us`)
- ⏳ **Key expiry**: the `EXPIRE`/`TTL` family and `SET` options with Redis 7's rules; expired keys are removed when touched and by a background cycle, like in Redis
- 🔐 **Authentication**: `--requirepass`, with `AUTH` and `HELLO ... AUTH`
- 🧭 **Introspection**: `INFO`, `CONFIG GET`, `COMMAND` and `CLIENT` answer the way client libraries and tools expect
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
- **Concurrent storage**: the keyspace is split into 1024 hashbrown tables, each behind its own read-write lock; a key is hashed once (SipHash with random keys) to pick its shard and its slot. A command on several keys (`MSET`, `MGET`, `DEL`, `EXISTS`, `RENAME`) locks all of their shards together, always in ascending order, so like in Redis it is atomic. Canonical integers are stored as numbers, everything else byte for byte.
- **Expiry**: an expiry is an absolute time stored with the value, checked on every access; each shard counts its keys with an expiry, and a background thread sweeps the shards where keys may have expired ten times a second, for at most 25 ms, like Redis's active expiry cycle.
- **AOF persistence**: dedicated writer thread fed by a bounded channel (back-pressure); queued records are written in batches and synced at most once per second. Each record is queued while the keys it changes are still locked, so the records of a key are in the order its changes were applied.

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
cargo run --release -- --backend uring
# Listen on localhost:6380 only, and require a password
cargo run --release -- --bind 127.0.0.1 --port 6380 --requirepass secret
```

The server will start on `0.0.0.0:7379` by default; `-- --help` lists the options.

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
| `ECHO message` | Reply with the message | `ECHO hi` → `$2\r\nhi` |
| `SELECT index` | Select a database; only database 0 exists | `SELECT 0` → `+OK` |
| `QUIT` | Reply, then close the connection; requests sent after it are dropped | `QUIT` → `+OK` |
| `HELLO [protover [AUTH username password] [SETNAME clientname]]` | Switch to RESP2 or RESP3 and describe the server | `HELLO 3` → `%7\r\n$6\r\nserver...` |
| `AUTH [username] password` | Authenticate the connection when the server runs with `--requirepass` (the only user is `default`) | `AUTH secret` → `+OK` |
| `COMMAND [COUNT\|INFO [name ...]\|LIST [FILTERBY ...]\|GETKEYS command [arg ...]\|HELP]` | Describe the commands: arity, flags, key positions and ACL categories as Redis 7.0 reports them (without key specifications); `COMMAND DOCS` is not supported, so `redis-cli` uses its own hints | `COMMAND COUNT` → `:44` |
| `CLIENT ID\|GETNAME\|SETNAME\|SETINFO\|HELP` | The connection's id and name, the client library's name and version | `CLIENT SETNAME app` → `+OK` |
| `INFO [section ...]` | Server, clients, persistence, stats, replication and keyspace sections, in Redis's format | `INFO keyspace` → `# Keyspace\r\ndb0:keys=2,...` |
| `CONFIG GET parameter [parameter ...]` | Configuration parameters matching names or glob patterns | `CONFIG GET save` → `*2\r\n$4\r\nsave\r\n$0\r\n` |
| `SET key value [NX\|XX] [GET] [EX\|PX\|EXAT\|PXAT time\|KEEPTTL]` | Set a value, if the condition holds, with an expiry | `SET key value EX 60` → `+OK` |
| `SETEX`/`PSETEX key time value` | Set a value that expires after `time` seconds or milliseconds | `SETEX key 60 value` → `+OK` |
| `SETNX key value` | Set a value if the key does not exist | `SETNX key value` → `:1` |
| `GET key` | Get a value | `GET key` → `$5\r\nvalue` |
| `GETSET key value` | Set a value and return the old one | `GETSET key new` → `$5\r\nvalue` |
| `GETDEL key` | Get a value and delete the key | `GETDEL key` → `$3\r\nnew` |
| `GETEX key [EX\|PX\|EXAT\|PXAT time\|PERSIST]` | Get a value and change its expiry | `GETEX key EX 60` → `$5\r\nvalue` |
| `DEL key [key ...]` | Delete keys, reply with the number removed | `DEL a b` → `:2` |
| `UNLINK key [key ...]` | Delete keys, like `DEL` | `UNLINK a b` → `:2` |
| `EXISTS key [key ...]` | Count how many of the keys exist | `EXISTS a b` → `:1` |
| `INCR key` | Increment an integer | `INCR counter` → `:1` |
| `INCRBY key increment` | Add to an integer | `INCRBY counter 5` → `:6` |
| `DECR key` | Decrement an integer | `DECR counter` → `:5` |
| `DECRBY key decrement` | Subtract from an integer | `DECRBY counter 2` → `:3` |
| `RENAME key newkey` | Rename a key | `RENAME old new` → `+OK` |
| `MGET key [key ...]` | Get multiple values | `MGET k1 k2` → `*2\r\n...` |
| `MSET key value [key value ...]` | Set multiple values | `MSET k1 v1 k2 v2` → `+OK` |
| `MSETNX key value [key value ...]` | Set multiple values if none of the keys exists | `MSETNX k1 v1 k2 v2` → `:1` |
| `TYPE key` | Type of the value (`string`), or `none` | `TYPE k1` → `+string` |
| `EXPIRE`/`PEXPIRE key time [NX\|XX\|GT\|LT]` | Expire a key after `time` seconds or milliseconds | `EXPIRE k1 60` → `:1` |
| `EXPIREAT`/`PEXPIREAT key time [NX\|XX\|GT\|LT]` | Expire a key at a unix time in seconds or milliseconds | `EXPIREAT k1 4102444800` → `:1` |
| `TTL`/`PTTL key` | Time left before the key expires (`-1` without expiry, `-2` if missing) | `TTL k1` → `:60` |
| `EXPIRETIME`/`PEXPIRETIME key` | The key's expiry as a unix time | `EXPIRETIME k1` → `:4102444800` |
| `PERSIST key` | Remove the key's expiry | `PERSIST k1` → `:1` |
| `DBSIZE` | Number of keys | `DBSIZE` → `:2` |
| `KEYS pattern` | Keys matching a glob pattern (`*`, `?`, `[a-z]`, `\\`), like Redis | `KEYS user:*` → `*2\r\n...` |
| `SCAN cursor [MATCH pattern] [COUNT count] [TYPE type]` | Iterate over the keys | `SCAN 0` → `*2\r\n$2\r\n17\r\n*...` |
| `FLUSHDB [ASYNC\|SYNC]`, `FLUSHALL [ASYNC\|SYNC]` | Delete every key (with `ASYNC` the memory is freed in the background) | `FLUSHDB` → `+OK` |

Replies and error messages match Redis 7, for example `-ERR wrong number of arguments for 'get' command`, `-ERR unknown command 'FOO', with args beginning with: ...` and `-ERR value is not an integer or out of range`. After an invalid command the connection keeps working; after malformed RESP the server replies `-ERR Protocol error: ...` and closes the connection, as Redis does.

## 🔧 Configuration

### Command-line Options

- `--bind ADDR`: the IPv4 or IPv6 address to listen on (default `0.0.0.0`, every IPv4 address); `--bind 127.0.0.1` keeps the server local.
- `--port N`: the TCP port (default `7379`).
- `--backend uring`: use the io_uring backend (Linux only; elsewhere Ignix falls back to mio).
- `--requirepass PASSWORD` (or `--requirepass=PASSWORD`): clients must authenticate with `AUTH PASSWORD`, `AUTH default PASSWORD` or `HELLO 3 AUTH default PASSWORD` before running other commands, as with Redis `requirepass`; the others get `-NOAUTH Authentication required.`. The password is compared in constant time. Clients pass it as usual, for example `redis-cli -a PASSWORD` or `redis://:PASSWORD@host:7379`.
- `--busy-poll-us N`: how long a worker of the default (mio) backend keeps polling for new events after its last one before it sleeps, in microseconds; the default is 50 and `0` disables busy-polling. It trades CPU time for latency: an idle server uses no CPU either way, at 1,000 requests per second the server used about 10% of a CPU instead of 5% in our measurements, and under sustained load every busy worker uses a full core.
- `-h`/`--help` and `-V`/`--version`.

Every option also takes the form `--option=value`. An unknown option or an invalid value prints the usage to stderr and exits with code 2.

### Environment Variables

- `RUST_LOG`: log level for diagnostics (`error`, `warn`, `info`, `debug`); the default is `info`.

### AOF Persistence

Ignix appends every command that changes data to `ignix.aof` in its working directory, in RESP format, and syncs it to disk at most one second after a write. Like Redis, it logs times as absolute: `SETEX` and a `SET` with an expiry become `SET key value PXAT <time>`, the `EXPIRE` family becomes `PEXPIREAT`, and `GETDEL`, `UNLINK` and keys that expire become `DEL`; commands that change nothing are not logged. The file is not loaded on startup yet, so data does not survive a restart. If the file cannot be opened, Ignix logs a warning and runs without persistence.

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
127.0.0.1:7379> SET session token EX 60
OK
127.0.0.1:7379> TTL session
(integer) 60
```

With `--requirepass`, pass the password with `redis-cli -a PASSWORD` (or `AUTH PASSWORD` in the
session). Interactive `redis-cli` shows its usual command hints.

### Using Any Redis Client Library

Ignix speaks RESP2 and, after `HELLO 3`, RESP3, so clients work with their default settings:

```python
import redis

# Connect to Ignix (add password='...' if it runs with --requirepass)
r = redis.Redis(host='localhost', port=7379, decode_responses=True)

# Use like Redis
r.set('hello', 'world')
print(r.get('hello'))  # Output: world
```

See [examples/](examples/) for complete Rust, Python and Node.js clients.

## 📊 Performance

Measured on Ignix v0.4.0 on a shared cloud VM: 4 vCPUs (Intel Xeon @ 2.10 GHz, one thread per core), 15 GB RAM, Linux 6.18, with servers and clients on the same machine. Redis 7.0.15 ran with `--appendonly yes --appendfsync everysec --save ""`, so both servers append every write to a file and sync it about once per second. Ignix used its defaults: one worker thread per core and a 50 µs busy-poll window; the "no busy-poll" column is `--busy-poll-us=0`. Redis executes commands on one thread. The servers ran one after another in two rounds (in alternating order, with a 3 s pause between each SET and GET run); cells show `round 1 / round 2`.

### redis-benchmark, one client thread

`redis-benchmark -t set,get -c 50 -n 500000 -d 64` (or `-d 1024`), and `-n 1000000 -P 16` (16 pipelined requests per connection); nothing pinned to CPUs, 4 Ignix workers:

| Test | Redis (req/s) | Ignix (req/s) | Ignix/Redis | Ignix, no busy-poll |
|------|---------------|---------------|-------------|---------------------|
| SET 64 B | 96,843 / 105,485 | 115,929 / 113,482 | 1.20x / 1.08x | 79,962 / 74,906 |
| GET 64 B | 99,128 / 98,155 | 115,393 / 111,657 | 1.16x / 1.14x | 79,466 / 77,930 |
| SET 1 KB | 96,006 / 106,045 | 84,147 / 101,276 | 0.88x / 0.96x | 63,581 / 72,527 |
| GET 1 KB | 98,990 / 96,581 | 110,791 / 110,742 | 1.12x / 1.15x | 81,473 / 82,115 |
| SET 64 B, pipelined | 769,823 / 650,195 | 1,560,062 / 1,261,034 | 2.03x / 1.94x | 1,064,963 / 1,043,841 |
| GET 64 B, pipelined | 1,223,990 / 1,394,700 | 1,584,786 / 1,557,632 | 1.29x / 1.12x | 1,160,093 / 1,169,591 |

With a single client thread on a 4-vCPU machine the client is the bottleneck, and a large part of each request's cost is waking the server thread that handles it. Busy-polling keeps Ignix's workers awake while requests keep coming: with it, Ignix serves 8-20% more requests than Redis without pipelining, except for 1 KB SETs (4-12% fewer), and 1.1-2.0x as many with pipelining; without it, Ignix is ahead only for pipelined SETs. These runs vary a lot because the busy-polling workers share the four CPUs with the client.

### redis-benchmark, server-bound

Both servers pinned to CPUs 0-1 (`taskset -c 0-1`, so Ignix runs 2 workers) and `redis-benchmark --threads 2 -c 50 -n 1000000 -d 64` on CPUs 2-3:

| Test | Redis (req/s) | Ignix (req/s) | Ignix/Redis | Ignix, no busy-poll |
|------|---------------|---------------|-------------|---------------------|
| SET 64 B | 124,860 / 133,298 | 210,393 / 199,920 | 1.69x / 1.50x | 159,795 / 159,923 |
| GET 64 B | 147,995 / 142,735 | 210,438 / 210,438 | 1.42x / 1.47x | 148,060 / 148,104 |

When the client is not the bottleneck, Ignix's two workers serve 1.4-1.7x as many requests as Redis's single thread on the same two CPUs, with less than half the median latency (0.11-0.12 ms against 0.26-0.32 ms). Ignix's GET runs stopped at about 210,000 requests per second in both rounds, the most the two client threads reached in any run, so for GET the client still limits the ratio.

### Python suite (`benchmarks/`)

`benchmarks/scripts/comprehensive_benchmark.py` and `real_world_benchmark.py`, with both servers running at the same time; every reply is checked and all runs had 0 errors. The client is Python with one thread per connection, so these numbers show the client's limit more than the servers'.

| Operation | Size | Conns | Redis (ops/s) | Ignix (ops/s) | Ignix/Redis |
|-----------|------|-------|---------------|---------------|-------------|
| SET | 64 B | 50 | 14,972 / 14,492 | 15,699 / 15,773 | 1.05x / 1.09x |
| GET | 64 B | 50 | 15,402 / 15,175 | 15,135 / 13,421 | 0.98x / 0.88x |
| SET | 1 KB | 50 | 14,605 / 15,034 | 14,729 / 16,187 | 1.01x / 1.08x |
| GET | 1 KB | 50 | 14,850 / 13,252 | 16,212 / 16,645 | 1.09x / 1.26x |
| SET | 32 KB | 20 | 12,696 / 13,585 | 11,624 / 12,121 | 0.92x / 0.89x |
| GET | 32 KB | 20 | 12,436 / 12,563 | 13,345 / 12,294 | 1.07x / 0.98x |
| SET | 256 KB | 20 | 1,631 / 2,429 | 3,355 / 8,606 | 2.06x / 3.54x |
| GET | 256 KB | 20 | 5,301 / 3,803 | 5,571 / 5,257 | 1.05x / 1.38x |
| SET | 2 MB | 10 | 67 / 108 | 181 / 344 | 2.71x / 3.19x |
| GET | 2 MB | 10 | 1,294 / 1,117 | 1,152 / 1,168 | 0.89x / 1.05x |

Real-world scenario (session store: 80% GET / 20% SET, 10,000 keys, Zipfian access, 1-2 KB values, 50 connections):

| Metric | Redis | Ignix | Ignix/Redis |
|--------|-------|-------|-------------|
| Throughput | 14,668 / 14,682 ops/s | 14,687 / 15,146 ops/s | 1.00x / 1.03x |
| Avg latency | 3.34 / 3.32 ms | 3.29 / 3.20 ms | 0.99x / 0.96x |
| p99 latency (GET) | 10.3 / 10.0 ms | 11.8 / 11.7 ms | 1.14x / 1.17x |

Ignix is 2-3.5x ahead on large writes (256 KB and 2 MB values), where Redis also rewrites its AOF in the background. For the other operations it is between 12% behind and 38% ahead of Redis, and up to 32 KB both servers stay near the client's limit of about 15,000 operations per second. The real-world scenario runs at the same speed on both, with Ignix's GET p99 about 15% higher. An earlier measurement of the previous code saw the second real-world round, run right after the large-value runs had appended about 6 GB to Ignix's AOF, fall to 0.41x of Redis's throughput; it has not reappeared since, including in the second round above, so its cause is not known. Ignix's AOF grew to 12 GB over the two rounds, as it is never compacted; compacting it and taking the fsync off the AOF writer's path are planned. The earlier tables for v0.3.1 could not be reproduced: the scripts that produced them did not check GET replies or count errors.

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
├── bin/ignix.rs        # Server binary and its command line
├── lib.rs              # Library exports
├── protocol.rs         # RESP parser, command validation, reply writers
├── commands.rs         # Command table: names, arity, flags, key positions
├── session.rs          # Per-connection state: protocol, name, authentication
├── shard.rs            # Command execution logic
├── storage.rs          # In-memory storage (Dict) and expiry
├── glob.rs             # KEYS and SCAN patterns
├── info.rs             # INFO, CONFIG GET and COMMAND replies
├── stats.rs            # Counters reported by INFO
├── net.rs              # mio networking and event loop
├── net_uring.rs        # io_uring backend (Linux)
└── aof.rs              # AOF persistence

examples/               # Rust, Python and Node.js clients

tests/
├── common/             # Helpers that run commands from RESP requests
├── basic.rs            # Basic command flow
├── commands.rs         # Command semantics, checked against Redis replies
├── aof.rs              # AOF encoding and logging
├── protocol_framing.rs # RESP framing and limits
├── protocol_api.rs     # Request parsing and reply APIs
├── resp.rs             # Protocol parsing
├── server_flags.rs     # The server's command line (one test starts a server)
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

- Loading the AOF on startup and compacting it, taking the fsync off the AOF writer's path
- More Redis data types (HASH/LIST/SET), transactions (`MULTI`/`EXEC`) and Lua scripting
- RDB snapshots, metrics/monitoring
- Clustering and replication

## 🐛 Known Limitations

- Limited command set compared with Redis; `CLIENT` supports only `ID`, `GETNAME`, `SETNAME`, `SETINFO` and `HELP`, and `CONFIG` only `GET` (a fixed set of parameters, such as `save`, `appendonly`, `maxmemory` and `port`) and `HELP`.
- `INFO` has no memory or CPU sections.
- `SCAN` visits the keyspace one shard (1/1024 of the keys) at a time, so with many keys a step returns more keys than `COUNT`; like in Redis, every key that exists during the whole iteration is returned, and here exactly once.
- A single database: `SELECT` accepts only index 0.
- No inline commands (plain text lines such as `PING` typed into telnet); requests must be RESP arrays.
- Expired keys are removed when a command touches them and by a background cycle ten times a second, as in Redis; until then they still count in `DBSIZE`.
- The AOF is write-only: it is not loaded on startup, so data does not survive a restart, and it is never compacted, so it grows with every write. In one of our benchmark sessions `ignix.aof` grew to 13.8 GB while Redis, which rewrites its AOF, used 2.3 GB.
- By default the server listens on every IPv4 address, and without `--requirepass` any client can use it; do not expose it to untrusted networks. There are no ACL users besides `default`, no TLS, and one address per server (`--bind`).
- The mio backend listens with `SO_REUSEPORT`, so a second Ignix started on the same port shares it without an error.
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