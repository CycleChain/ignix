# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security
- **Remote crash on malformed requests**: a negative or oversized bulk length (e.g. `*1\r\n$-5\r\n`) made the parser overflow, and a huge element count (`*9223372036854775807\r\n`) made it try to allocate that many elements. Because the release profile uses `panic = "abort"`, a single packet from any client terminated the whole server. Lengths are now parsed with checked arithmetic, bulk strings are limited to 512 MiB and element counts to `INT_MAX` (as in Redis), and malformed input is answered with `ERR Protocol error: ...`.

- **One large request could stall a worker for seconds**: every read parsed a partial request again from its start and copied all of its arguments again, so a request arriving in many pieces took quadratic time. A 2.5 MB `DEL` with 200,000 keys arriving in 4 KiB reads kept a worker busy for 4.6 s (release build), during which its other connections waited. Each connection now keeps a `RequestParser` that continues where it stopped; the same request takes 31 ms.

### Added
- **`INFO` and `CONFIG GET`**: `INFO [section ...]` writes the server, clients, persistence, stats, replication and keyspace sections in Redis's format and order (a verbatim string in RESP3), reporting `redis_version:7.0.0` with Ignix's own version as `ignix_version`, uptime, port, event API, connected and total connections, executed commands and keys. `CONFIG GET` reports a fixed set of parameters (`save`, `appendonly`, `appendfsync`, `maxmemory`, `maxmemory-policy`, `port`, `bind`, `dir`, ...) by name or glob pattern, ignoring case like Redis, so `redis-benchmark` no longer warns that it cannot read the configuration. New API: `Stats`, reachable as `Shard::stats`, with the counters; each worker thread counts its commands in a counter of its own, so counting adds no shared write to a request.
- **`KEYS` and `SCAN`**: glob patterns match exactly what Redis 7.0 matches, including its quirks (character ranges compare signed bytes, an unclosed `[` runs to the end of the pattern, a pattern gives up after 1,000 stars, only `*` matches the empty key), but the matcher does not recurse, so a hostile 64 KiB pattern costs at most pattern length × key length steps. `KEYS` looks at every shard at once. `SCAN` parses its cursor and `MATCH`, `COUNT` and `TYPE` options like Redis, and its cursor is the index of the next of the 1024 shards: every key that exists during the whole iteration is returned exactly once, and with many keys a step returns about 1/1024 of them whatever `COUNT` says.
- **`DBSIZE`, `TYPE`, `UNLINK`, `FLUSHDB` and `FLUSHALL`**, with Redis's replies and errors. `TYPE` reports `string` for every stored value. `UNLINK` deletes like `DEL` and is logged to the AOF as `DEL`; `FLUSHDB` and `FLUSHALL` (Ignix has one database) accept `ASYNC` to free the memory in a background thread and `SYNC`, lock the keyspace only while its tables are swapped for empty ones, and are logged to the AOF. New API: `Cmd::DbSize`, `Cmd::Type`, `Cmd::Unlink`, `Cmd::FlushDb`, `Cmd::FlushAll`, `FlushMode`, `emit_aof_flushdb` and `emit_aof_flushall`.
- **`CLIENT ID`, `GETNAME`, `SETNAME`, `SETINFO` and `HELP`**: the connection's id and name (names follow Redis's rules; an empty name removes it) and the client library's name and version, which redis-py and node-redis send with `CLIENT SETINFO` when they connect, so their handshake no longer gets errors and node-redis needs no `disableClientInfo`. Unknown subcommands and wrong argument counts get Redis's errors (`ERR unknown subcommand 'FOO'. Try CLIENT HELP.`, `... for 'client|setname' command`). `SETINFO` comes from Redis 7.2. New API: `Session::lib_name`, `Session::lib_ver`, `ClientInfo` and the `Cmd::Client*` variants.
- **`HELLO` and RESP3**: `HELLO 3` switches a connection to RESP3 and `HELLO 2` back; `HELLO` replies with the server description Redis sends (`server` `redis`, `version` `7.0.0`, the client `id`, `mode`, `role`, `modules`), as a map in RESP3 and a flat array in RESP2. In RESP3 a missing key is the null `_` instead of `$-1`. `AUTH` and `SETNAME` options are applied in order, as Redis does; without a configured password only the user `default` is accepted. Clients that default to RESP3, such as redis-py 8, now work without `protocol=2`. New API: `Protocol`, `Session::id`, `Session::protocol`, `Session::name`, `Cmd::Hello`, `write_nil` and `write_map_len`.
- **`ECHO`, `QUIT` and `SELECT`**: `QUIT` replies `OK` and closes the connection once the reply is sent, dropping any requests pipelined after it, as Redis does. `SELECT 0` succeeds and any other index fails with `ERR DB index is out of range`, because Ignix has one database; non-integer and out-of-range indexes get Redis's errors. New API: `Session` (the state of one connection) and `Shard::exec_session`, which runs a command for a connection; `Shard::exec` runs it on a fresh session. New `Cmd` variants `Echo`, `Quit` and `Select`.
- **`INCRBY`, `DECRBY` and `DECR`**, with the same integer rules and error messages as Redis. redis-py implements `incr()` with `INCRBY`, so counters did not work with it before. New API: `Cmd::IncrBy(key, delta)` (DECRBY and DECR are parsed as negative deltas), `Dict::incr_by` and `emit_aof_incrby`.
- **Multi-key `DEL` and `EXISTS`, `PING [message]`**: `DEL` removes every given key and replies with the number removed, `EXISTS` counts every existing argument (repeated keys count each time), and `PING message` echoes the message as a bulk string, as in Redis.
- **Busy-polling** (mio backend): after its last event a worker keeps polling for new events without blocking for 50 µs before it sleeps, because waking a sleeping worker cost the client more than serving the request. `--busy-poll-us=N` sets the window and `--busy-poll-us=0` disables it; an idle server still sleeps. Library users can pass `ServerOptions` (field `busy_poll`, default `DEFAULT_BUSY_POLL`) to the new `run_server`; `run_shard` uses the defaults.
- **`AofHandle::write_owned`**: hands an encoded record (such as the result of `emit_aof_set`) to the writer thread without the copy that `write(&[u8])` makes.
- **`RequestParser`**: an incremental request parser for servers reading from sockets; it remembers how far a partial request was parsed, so each call only looks at new bytes. `parse_requests` is unchanged and still parses a partial request from its start on every call.
- **Request API for servers**: `protocol::parse_requests` parses every complete request into `Request::Cmd` or `Request::Invalid(error_line)` and only fails on protocol errors, and `protocol::write_error` writes a RESP error reply (`-ERR ...`).

### Changed
- **Own sharded keyspace instead of DashMap**: `Dict` now keeps the keys in 1024 hashbrown tables with one read-write lock each and hashes a key once to find both its shard and its slot; the `dashmap` dependency is gone. Behaviour and the public `Dict` methods are unchanged, and `Dict` gains `len`, `is_empty` and `clear`. The shards can be locked together in a fixed order, which later multi-key commands need.
- **Breaking: `Shard` has a new public field** `stats`, so a `Shard` can no longer be written as a struct literal; create it with `Shard::new`.
- **`Dict::len`, `is_empty` and `clear` see the keyspace at one moment**: they lock every shard, so a key moving between shards is counted once and `clear` removes every key at once; `clear` frees the removed keys after releasing the locks.
- **Multi-key commands are atomic**: `MSET`, `DEL`, `MGET` and `EXISTS` lock the shards of all their keys together (write locks for `MSET` and `DEL`, read locks for `MGET` and `EXISTS`, always in ascending shard order, so they cannot deadlock), so like every Redis command they run atomically. Before, each key was locked on its own: two concurrent `MSET a 1 b 1` and `MSET a 2 b 2` could leave `a` at 1 and `b` at 2, a `DEL a b` racing an `MSET a 1 b 1` could leave just one of the keys, and `MGET` could return some values from before and some from after a concurrent write. Single-key commands are unaffected. Criterion medians of five alternating runs: `exec/mset_10x1k` (overwriting 10 keys) and `exec/mget_16x1k` did not change beyond the noise, while `exec/mset_del_10x1k` (setting 10 new keys, then deleting them) became 22-27% slower, about 0.2 µs per 10-key command.
- **`parse_many` no longer gets stuck on an invalid command**: the invalid request is consumed before the error is returned, so the next call continues with the following request. Protocol errors still leave the malformed bytes in the buffer.
- **Breaking: command enum shapes**: `Cmd::Ping` is now `Cmd::Ping(Option<Bytes>)`, `Cmd::Del(Bytes)` is `Cmd::Del(Vec<Bytes>)` and `Cmd::Exists(Bytes)` is `Cmd::Exists(Vec<Bytes>)`. `Cmd` and `Value` are `#[non_exhaustive]`, so future commands and value types are not breaking changes.
- **Breaking: `Dict::incr`** is now `fn incr(&self, key: Bytes) -> Result<i64, IncrError>` (was `fn incr(&self, k: &[u8]) -> i64`); it takes the key by value, so no copy is made, and reports the new `IncrError`.
- **Redis-style argument validation**: a wrong number of arguments is rejected with `ERR wrong number of arguments for '<command>' command`, an unknown command with `ERR unknown command '<name>', with args beginning with: ...` (truncated like Redis), and `SET` options that are not implemented yet (`NX`, `XX`, `GET`, `EX`, `PX`, `EXAT`, `PXAT`, `KEEPTTL`) with `ERR SET option '<name>' is not supported`; any other extra `SET` token is `ERR syntax error`.
- **Logging**: diagnostics (accept errors, stopped workers, backend fallback) go through the `log` crate, so `RUST_LOG` now controls them; the default level is `info`. Previously nothing was logged and `RUST_LOG` had no effect.
- **Performance numbers re-measured**: the README tables for v0.3.1 could not be reproduced (their scripts did not check replies or count errors) and were replaced with measurements from `redis-benchmark` (one client thread, pipelined, and a server-bound setup with two client threads; with and without busy-polling) and the corrected Python suite against Redis 7.0.15 with AOF enabled, including the runs where Ignix is slower. README, `examples/README.md` and `HOW_TO_VERIFY_CONNECTION.md` now describe the actual commands, errors and limitations (RESP2 only, AOF not loaded on startup).
- **Stricter request framing**: length lines are parsed like Redis `string2ll` (no leading zeros, `+`, spaces or lines longer than 20 characters). Unlike Redis, a bulk payload must be followed by CRLF instead of skipping two bytes blindly, which stops a miscounted length from desynchronising the stream. Empty requests (`*0\r\n`, `*-1\r\n`) are ignored like in Redis instead of being an error.

### Performance
- **GET and MGET copy small values under the shard lock**: instead of cloning the stored `Bytes` (two atomic reference-count updates per read, contended when many connections read the same key), values up to 16 KiB are copied into the reply while the shard's read lock is held; larger values are still cloned and written after the lock is released. Criterion medians of five alternating runs: `exec/get_str_10k` -18%, `exec/mget_16x1k` -18%; `redis-benchmark` GET with one client thread +15% / +4% in two rounds.
- **Allocation-free reply headers**: `write_bulk`, `write_integer` and `write_array_len`, and GET/MGET of integer values, formatted numbers with `to_string()` (one heap allocation per reply); they now format into a stack buffer. Criterion on a shared 4-vCPU Linux container, two runs against the previous commit: `reply/write_bulk_10k` -22% / -25%, `reply/write_integer_10k` -20% / -22%, `reply/write_array_len_10k` -28% / -29%, `exec/get_int_10k` -5% / -15%; the unchanged SET and parser paths moved within the run-to-run noise (up to ±20%).
- **Busy-polling instead of sleeping between requests**: `redis-benchmark -c 50` against a server whose workers slept after every request spent most of its time waking them; with a 50 µs busy-poll window throughput rose by 6–46% (median 30%) without pipelining (one benchmark client thread, 1–4 workers), by 40–48% with two client threads and by 30–40% for pipelined GET, while pipelined SET stayed within the noise; two rounds on the shared 4-vCPU VM. The price is CPU time: an idle server still uses none, but at 1,000 requests per second the server used 10% of a CPU instead of 5%.
- **Batched AOF writes**: the writer thread wrote every record with its own `write` call and, between records, spun and yielded (`sched_yield`) or had to be woken by the worker that sent the next one, which cost the workers more CPU than the rest of a SET. It now collects every queued record into one write (up to 1 MiB) and, once the queue is empty, pauses for 200 µs, so senders no longer wake it and the next write covers many records. With busy-polling on, SET throughput rose by 10–15% with three workers, by 23–35% in the unpinned README setup and by 35–60% pipelined, and SET p99 latency with three or four workers fell from 4.3–4.8 ms to 0.8–0.9 ms (two rounds on the shared 4-vCPU VM); records reach the file at most about 0.2 ms later.
- **One `recv` per request instead of two** (mio backend on Linux): the read loop stopped only at `WouldBlock`, so every readable event ended with a `recvfrom` that failed with `EAGAIN`. Under edge-triggered epoll a short read already means the socket is drained, so the loop now stops there, except when the event reports that the peer closed (a FIN that arrived with the data is not reported again). Measured with `redis-benchmark` (`-c 50`, 64-byte values) on the shared 4-vCPU VM: 2.0 → 1.0 `recvfrom` per request (no `EAGAIN`), worker CPU per GET -3% to -9%; throughput did not change beyond the run-to-run noise (±5–10%).

### Fixed
- **Connections stuck after an invalid request**: an unknown command or malformed request stayed at the head of the read buffer, so every later request on that connection got the same error, and requests parsed before it in the same read were dropped without being executed or answered. Clients that send `CLIENT SETINFO` on connect (redis-py 5, node-redis 4.7) were affected. Invalid commands now get an error reply and the connection keeps working; after a protocol error the error is sent and the connection is closed, as in Redis.
- **Errors sent as status replies**: request errors were written as `+ERR ...`, which clients read as success; they are now RESP errors (`-ERR ...`).
- **Requests dropped when a client half-closes**: when data and EOF arrived in the same read, the connection was closed without running the requests; they now run and their replies are flushed before the connection is closed.
- **io_uring backend** (`--backend=uring`): closed connections leaked their socket (500 connections left 500 open descriptors), one failed accept stopped the server from accepting new connections for good, invalid requests got no reply so clients hung, a full submission queue or `EINTR` aborted the process, and IPv6 peers overflowed the 16-byte accept address buffer. Sockets are now owned and closed with their connection, accept is re-armed after errors (pausing 10 ms when out of descriptors or memory), requests are handled exactly like the mio backend, and submissions that do not fit are queued.
- **One connection could stop a worker thread**: a registration error at accept or an interrupted `poll` ended the whole worker and its connections.
- **Unusable AOF path aborted the server**: the AOF file was opened inside the writer thread with `expect`, so with `panic = "abort"` a release server started where `ignix.aof` could not be created terminated immediately (debug builds silently ran without persistence). `spawn_aof_writer` now returns the error; the server logs `AOF persistence disabled: ...` and keeps serving.
- **AOF not synced while idle**: written records were only synced to disk when another write arrived at least a second later, so the last writes before a quiet period could stay unsynced indefinitely. The writer now wakes up when the next sync is due, so written data is synced within about a second even when writes stop, and write errors are logged instead of ignored.
- **AOF corrupted binary data and missed deletes**: records were built with `String::from_utf8_lossy`, so a key or value that was not valid UTF-8 was written with different bytes than its declared length and broke the framing of the whole file, and `DEL` was never logged, so replaying the file would bring deleted keys back. Records are now encoded byte for byte, and `DEL` is logged with the keys it actually removed (new `emit_aof_del`).
- **`RENAME` was not atomic**: the key was removed under its old name before it was stored under the new one, so for a moment it existed under neither name, and a client could find the old name gone and then the new name still missing. Both names are now locked together.
- **`RENAME` errors**: renaming a missing key replied `+ERR no such key`, a status reply clients read as success; it is now the RESP error `-ERR no such key`. `RENAME k k` replied `OK` even when `k` did not exist; like Redis it now fails with `ERR no such key`.
- **`INCR` overwrote non-numeric values and overflowed**: `INCR` on a value such as `abc` replaced it with `1`, and at `9223372036854775807` it panicked (debug builds) or wrapped around (release). It now fails with `ERR value is not an integer or out of range` or `ERR increment or decrement would overflow` and leaves the value unchanged; failed increments are no longer written to the AOF.
- **`SET`/`MSET` changed some values**: every value that parsed as an integer was stored as a number, so `SET k 007` followed by `GET k` returned `7`, and `-0` became `0`. Only canonical integers (as Redis defines them) are stored as numbers now; every other value is kept byte for byte.
- **Silently ignored arguments**: `DEL a b c` deleted only `a`, `EXISTS a b` checked only `a`, `GET a b` and `INCR a b` ignored the extra argument, and `SET k v NX` or `SET k v EX 10` set the key unconditionally and without expiry while replying `OK`.
- **Benchmark scripts reported unchecked results**: the Python clients counted a missing or error GET reply as a success, ignored failed pre-fills, never reported errors, and could desynchronise on large values (`basic_benchmark.py` read a bulk reply with a single `recv`); `basic_benchmark.py` only compared 1-connection runs. All scripts now share `benchmarks/scripts/resp_client.py`, check every reply, report errors (and exit non-zero on them), exclude connection setup from timing, and run each configuration on both servers back to back. `run_benchmarks.sh` and `run_tests.sh` no longer `pkill -9 ignix`, stop only what they started and propagate the exit status.
- **Client examples**: the redis-py example broke on connect and used `KEYS`; the node-redis example connected to port 6379 because node-redis 4 ignores top-level `host`/`port`, and used `KEYS`/`QUIT`. Both run end to end against Ignix now (redis-py 8.1.0, node-redis 4.7.1). The raw-socket examples (`simple_python_client.py`, `simple_nodejs_client.js`) printed `MGET` replies as raw RESP, sent bulk lengths in characters instead of bytes (breaking non-ASCII values), returned error replies as ordinary strings and could lose sync when a reply arrived in several reads; they now parse complete replies and raise errors.
- **Linux build**: the crate did not compile on Linux (and therefore not on docs.rs) because `src/net_uring.rs` used an `Ok(_)` pattern that resolved to `anyhow::Ok` (E0532) and borrowed the io_uring instance mutably twice (E0499).
- **Server started without listeners**: when the port could not be bound (for example because another process held it without `SO_REUSEPORT`), every worker logged an error and stopped, but the server printed its startup banner and exited with status 0 without ever accepting a connection. The listeners are now bound before the workers start; a failure is returned as `cannot listen on <addr>` and the process exits with status 1.
- **AOF after write and sync errors**: a write that failed partway (for example on a full disk) left part of a record in the file and later records were appended after it, which broke the framing of everything that followed; a failed fsync was treated as done and never retried. The writer now cuts the file back to the last complete record, as Redis does, appends nothing after a partial record it cannot remove, and retries a failed sync a second later.
- **API documentation**: RESP formats in doc comments, such as `$<len>\r\n<data>\r\n`, were read as HTML tags, so the placeholders vanished from the rendered docs; `cargo doc` now builds without warnings.

### Migration Notes
- Create shards with `Shard::new(id, aof)` instead of a struct literal.
- Call `dict.incr(Bytes::copy_from_slice(key))` (or pass an owned `Bytes`) and handle the `IncrError` result.
- Build `Cmd::Ping(None)`, `Cmd::Del(vec![key])` and `Cmd::Exists(vec![key])` where the old unit/single-key variants were used, and add a wildcard arm to exhaustive matches on `Cmd` and `Value`.
- Errors from `parse_one` and `parse_many` are now complete Redis error lines that already start with `ERR`; send them with `write_error(&e.to_string(), out)` instead of adding an `ERR` prefix and writing a status reply.

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
