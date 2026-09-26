# Redis vs Ignix Benchmarks

Python scripts that compare Ignix with Redis over the RESP protocol, plus a
recipe for the standard `redis-benchmark` tool.

## Contents

| File | Purpose |
|------|---------|
| `run_benchmarks.sh` | Builds Ignix in release mode, starts Ignix (and Redis, when port 6379 is free) in temporary directories, runs `run_all.py`, and stops only the processes it started |
| `run_all.py` | Runs the three benchmarks below against running servers and writes `results/` |
| `quick_benchmark.py` | 1000 SETs and 1000 GETs on one connection per server; a quick sanity check |
| `scripts/basic_benchmark.py` | SET/GET for chosen value sizes and connection counts |
| `scripts/comprehensive_benchmark.py` | SET/GET from 64 B to 2 MB values; JSON and charts |
| `scripts/real_world_benchmark.py` | Session-store mix: 80% GET / 20% SET, Zipfian keys, 1-2 KB values |
| `scripts/resp_client.py` | Shared minimal RESP client |
| `run_tests.sh` | Runs the large payload tests against a release build |

## Requirements

- Python 3.8+ (standard library only).
- Optional, for charts: `pip install matplotlib seaborn numpy pandas`.
- Redis on `localhost:6379` and Ignix on `localhost:7379`, or `redis-server`
  installed so `run_benchmarks.sh` can start it.

## Quick start

```bash
# Everything, including building and starting the servers
bash benchmarks/run_benchmarks.sh

# With both servers already running
python3 benchmarks/run_all.py
python3 benchmarks/quick_benchmark.py
```

The scripts check both servers with `PING` first and exit with a non-zero
status if a server is missing, a run fails, or any request got a wrong
reply.

## Script options

`scripts/basic_benchmark.py`

| Option | Default | Meaning |
|--------|---------|---------|
| `--data-sizes` | `64 256 1024 4096` | Value sizes in bytes |
| `--connections` | `1 10 50` | Concurrent connections |
| `--operations` | `1000` | Operations per connection |
| `--output-dir` | `benchmark_results` | Where JSON and charts go |
| `--skip-plots` | off | Do not create charts |

`scripts/comprehensive_benchmark.py` and `scripts/real_world_benchmark.py`

| Option | Default | Meaning |
|--------|---------|---------|
| `--target` | `all` | `all`, `redis` or `ignix` |
| `--out` | `comprehensive_results` / `real_world_results` | Chart directory |
| `--json-out` | `benchmark_results.json` / `real_world_results.json` | JSON results; entries for the same server and configuration are replaced, so separate `--target` runs combine |
| `--report-only` | off | Print the Markdown table from an existing JSON file |

The comprehensive configurations are fixed: 64 B and 1 KB values with 50
connections (10,000 measured operations), 32 KB and 256 KB with 20
connections (5,000), and 2 MB with 10 connections (1,000), each after a
warm-up. GET runs first write every key; with 2 MB values that is 2 GB per
server.

## What is measured

- **Every reply is checked.** SET must answer `OK`; GET must return exactly
  the value that was written (the real-world GETs must return a stored
  value). Anything else, including error replies, counts as an error and is
  reported in the output and the JSON files.
- **Timing.** Each request is timed with `time.perf_counter()`. Throughput
  is successful requests divided by the time from the first to the last
  request of the run; connecting and preparing requests are not timed.
- **Interleaving.** For each configuration the comprehensive and basic
  benchmarks run Redis and Ignix back to back, so slow drift of the machine
  affects both servers alike.

## Fair comparisons

- **Persistence.** Ignix always writes an append-only file and syncs it at
  most once per second. Run Redis with
  `--appendonly yes --appendfsync everysec --save ""` for the same
  guarantees; `run_benchmarks.sh` does this when it starts Redis.
- **Threads.** Ignix runs one event loop per CPU core; Redis executes
  commands on a single thread. Both share the machine with the client.
- **The client is the bottleneck.** These scripts use Python threads, which
  share one interpreter lock, so the numbers compare the servers under the
  same client rather than their maximum throughput. For server throughput
  use `redis-benchmark` from the Redis distribution:

  ```bash
  redis-benchmark -h 127.0.0.1 -p 7379 -t set,get -n 200000 -c 50 -d 64 -q
  redis-benchmark -h 127.0.0.1 -p 7379 -t set,get -n 1000000 -c 50 -P 16 -q
  ```

  Pass `-t set,get`: the default test list starts with `PING_INLINE`, and
  Ignix does not implement inline commands. `redis-benchmark` prints a
  warning about `CONFIG GET`, which Ignix does not support; the results are
  not affected.

## Output files

All results are written under `benchmarks/results/` (ignored by git):

- `basic/benchmark_results.json` and one set of charts per connection count;
- `comprehensive/benchmark_results.json`, `throughput.png`,
  `latency_dist.png`, `tail_latency_<op>_<size>B.png`;
- `real_world/real_world_results.json`, `real_world_throughput.png`,
  `real_world_latency.png`;
- `index.html`, linking the charts that were generated.

When a script is run on its own, `--json-out` and `--out` default to the
current directory.
