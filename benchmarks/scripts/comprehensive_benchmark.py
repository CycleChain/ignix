#!/usr/bin/env python3
"""SET/GET throughput and latency of Redis and Ignix across value sizes.

Every reply is checked (SET must answer OK, GET must return the exact value
that was written) and failures are counted as errors, printed and saved.
Throughput is measured over the request loops only; connecting is not
timed. For each configuration the servers run one after the other, so slow
drift of the machine affects both alike.
"""

import argparse
import json
import os
import random
import statistics
import string
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import List, Tuple

from resp_client import ProtocolError, RespClient, RespError, encode_command, ping

try:
    import matplotlib.pyplot as plt
    import seaborn as sns
    import numpy as np
    import pandas as pd
    HAS_PLOTTING = True
except ImportError:
    HAS_PLOTTING = False
    print("⚠️  matplotlib, seaborn, numpy, or pandas not found. Graphs cannot be created.")
    print("   Installation: pip install matplotlib seaborn numpy pandas")

KEYS = 1000
MAX_ERROR_SAMPLES = 5


@dataclass
class BenchmarkConfig:
    host: str
    port: int
    name: str
    warmup_ops: int
    measure_ops: int
    connections: int
    data_size: int
    operation: str


@dataclass
class BenchmarkResult:
    config: BenchmarkConfig
    latencies: List[float] = field(default_factory=list)
    start_time: float = 0.0
    end_time: float = 0.0
    errors: int = 0
    error_samples: List[str] = field(default_factory=list)

    @property
    def total_time(self):
        return self.end_time - self.start_time

    @property
    def ops_per_sec(self):
        return len(self.latencies) / self.total_time if self.total_time > 0 else 0

    @property
    def avg_latency(self):
        return statistics.mean(self.latencies) if self.latencies else 0

    def percentile(self, p):
        if len(self.latencies) < 100:
            return 0
        if len(self.latencies) >= 1000:
            return statistics.quantiles(self.latencies, n=1000)[int(p * 10) - 1]
        return statistics.quantiles(self.latencies, n=100)[int(p) - 1]


@dataclass
class WorkerResult:
    latencies: List[float] = field(default_factory=list)
    errors: int = 0
    error_samples: List[str] = field(default_factory=list)
    start: float = 0.0
    end: float = 0.0


def generate_data(size: int) -> bytes:
    return "".join(random.choices(string.ascii_letters + string.digits, k=size)).encode()


def run_worker(config: BenchmarkConfig, requests: List[bytes], expected: List[bytes],
               ops: int) -> WorkerResult:
    """Send `ops` requests on one connection and check every reply."""
    result = WorkerResult()
    client = RespClient(config.host, config.port)
    try:
        client.connect()
    except OSError as e:
        result.errors = ops
        result.error_samples.append(f"connect: {e}")
        return result

    try:
        result.start = time.perf_counter()
        for i in range(ops):
            index = i % len(requests)
            t0 = time.perf_counter()
            try:
                reply = client.request(requests[index])
            except RespError as e:
                reply = e
            except (OSError, ProtocolError, ValueError) as e:
                # The connection is unusable: count the rest as failed.
                result.errors += ops - i
                result.error_samples.append(f"{type(e).__name__}: {e}")
                break
            t1 = time.perf_counter()
            if reply == expected[index]:
                result.latencies.append((t1 - t0) * 1000.0)
            else:
                result.errors += 1
                if len(result.error_samples) < MAX_ERROR_SAMPLES:
                    result.error_samples.append(f"unexpected reply: {str(reply)[:80]}")
        result.end = time.perf_counter()
    finally:
        client.close()
    return result


def prefill(config: BenchmarkConfig, keys: List[bytes], value: bytes) -> None:
    """Write every key once so GET has something to read; fail loudly."""
    with RespClient(config.host, config.port, timeout=30.0) as client:
        for key in keys:
            reply = client.execute("SET", key, value)
            if reply != b"OK":
                raise RuntimeError(f"pre-fill SET {key!r} answered {reply!r}")


def run_phase(config: BenchmarkConfig, requests, expected, total_ops: int) -> List[WorkerResult]:
    ops = total_ops // config.connections
    with ThreadPoolExecutor(max_workers=config.connections) as ex:
        futures = [ex.submit(run_worker, config, requests, expected, ops)
                   for _ in range(config.connections)]
        return [f.result() for f in futures]


def benchmark(config: BenchmarkConfig) -> BenchmarkResult:
    print(f"🚀 Benchmarking {config.name} ({config.host}:{config.port})")
    print(f"   Op: {config.operation}, Size: {config.data_size}B, Conn: {config.connections}")

    keys = [f"key_{i}".encode() for i in range(KEYS)]
    value = generate_data(config.data_size)

    if config.operation == "GET":
        print("   📝 Pre-filling data...")
        prefill(config, keys, value)
        requests = [encode_command("GET", k) for k in keys]
        expected = [value] * KEYS
    else:
        requests = [encode_command("SET", k, value) for k in keys]
        expected = [b"OK"] * KEYS

    if config.warmup_ops > 0:
        print(f"   🔥 Warming up ({config.warmup_ops} ops)...")
        run_phase(config, requests, expected, config.warmup_ops)

    print(f"   ⏱️  Measuring ({config.measure_ops} ops)...")
    result = BenchmarkResult(config)
    workers = run_phase(config, requests, expected, config.measure_ops)
    for w in workers:
        result.latencies.extend(w.latencies)
        result.errors += w.errors
        result.error_samples.extend(w.error_samples[:MAX_ERROR_SAMPLES - len(result.error_samples)])
    timed = [w for w in workers if w.end > 0]
    if timed:
        result.start_time = min(w.start for w in timed)
        result.end_time = max(w.end for w in timed)

    print(f"   ✅ Done! {result.ops_per_sec:.1f} ops/sec, Avg Lat: {result.avg_latency:.3f}ms, "
          f"errors: {result.errors}")
    for sample in result.error_samples:
        print(f"      ⚠️  {sample}")
    print("-" * 60)
    return result


def plot_results(results: List[BenchmarkResult], output_dir: str):
    if not HAS_PLOTTING:
        print("⚠️  Skipping plot generation: matplotlib/seaborn/pandas not found.")
        return

    if not results:
        print("⚠️  Skipping plot generation: No benchmark results to plot.")
        return
    os.makedirs(output_dir, exist_ok=True)

    sns.set_theme(style="whitegrid")

    # 1. Throughput Comparison (Bar Chart)
    plt.figure(figsize=(12, 6))
    data = []
    for r in results:
        data.append({
            "Server": r.config.name,
            "Operation": f"{r.config.operation}\n{r.config.data_size}B",
            "Throughput": r.ops_per_sec
        })

    df = pd.DataFrame(data)
    sns.barplot(data=df, x="Operation", y="Throughput", hue="Server", palette="viridis")
    plt.title("Throughput Comparison (Ops/Sec) - Higher is Better")
    plt.ylabel("Operations / Second")
    plt.savefig(f"{output_dir}/throughput.png")
    plt.close()

    # 2. Latency Distribution (Box Plot)
    plt.figure(figsize=(12, 6))
    lat_data = []
    for r in results:
        # Downsample for plotting if too many points
        lats = r.latencies if len(r.latencies) < 10000 else random.sample(r.latencies, 10000)
        for l in lats:
            lat_data.append({
                "Server": r.config.name,
                "Scenario": f"{r.config.operation} {r.config.data_size}B",
                "Latency (ms)": l
            })

    lat_df = pd.DataFrame(lat_data)
    sns.boxplot(data=lat_df, x="Scenario", y="Latency (ms)", hue="Server", palette="viridis", showfliers=False)
    plt.title("Latency Distribution (Lower is Better)")
    plt.savefig(f"{output_dir}/latency_dist.png")
    plt.close()

    # 3. Latency Percentiles (Line Plot)
    percentiles = [50, 90, 95, 99, 99.9]
    p_data = []

    for r in results:
        if not r.latencies:
            continue
        sorted_lats = sorted(r.latencies)
        n = len(sorted_lats)
        for p in percentiles:
            idx = max(int(n * (p / 100.0)) - 1, 0)
            p_data.append({
                "Server": r.config.name,
                "Percentile": str(p),
                "Latency (ms)": sorted_lats[idx],
                "Scenario": f"{r.config.operation} {r.config.data_size}B"
            })

    # Plot separate charts per scenario for clarity
    scenarios = set(d["Scenario"] for d in p_data)
    for sc in scenarios:
        plt.figure(figsize=(10, 5))
        subset = [d for d in p_data if d["Scenario"] == sc]
        subset_df = pd.DataFrame(subset)
        sns.lineplot(data=subset_df, x="Percentile", y="Latency (ms)", hue="Server", marker="o")
        plt.title(f"Tail Latency - {sc} (Lower is Better)")
        plt.yscale("log")
        plt.savefig(f"{output_dir}/tail_latency_{sc.replace(' ', '_')}.png")
        plt.close()


def save_results_json(results: List[BenchmarkResult], filename: str):
    """Merge `results` into `filename`, replacing earlier entries for the same
    server and configuration, so separate `--target` runs can be combined
    without accumulating stale duplicates."""
    data = []
    if os.path.exists(filename):
        try:
            with open(filename, 'r') as f:
                data = json.load(f)
        except (OSError, ValueError):
            data = []

    measured_at = time.strftime("%Y-%m-%dT%H:%M:%S")
    for r in results:
        entry = {
            "name": r.config.name,
            "operation": r.config.operation,
            "data_size": r.config.data_size,
            "connections": r.config.connections,
            "ops_per_sec": r.ops_per_sec,
            "avg_latency": r.avg_latency,
            "p50": r.percentile(50),
            "p99": r.percentile(99),
            "successful_ops": len(r.latencies),
            "errors": r.errors,
            "measured_at": measured_at,
        }
        key = (entry["name"], entry["operation"], entry["data_size"], entry["connections"])
        data = [d for d in data
                if (d.get("name"), d.get("operation"), d.get("data_size"), d.get("connections")) != key]
        data.append(entry)

    os.makedirs(os.path.dirname(os.path.abspath(filename)), exist_ok=True)
    with open(filename, 'w') as f:
        json.dump(data, f, indent=2)
    print(f"   💾 Results saved to {filename}")


def generate_markdown_table(json_file: str):
    if not os.path.exists(json_file):
        print("No results file found.")
        return

    with open(json_file, 'r') as f:
        data = json.load(f)

    # Group by size, operation and connections
    grouped = {}
    for item in data:
        key = (item['data_size'], item['operation'], item.get('connections', 0))
        grouped.setdefault(key, {})[item['name']] = item

    print("\n### Benchmark Results Summary\n")
    print("| Operation | Size | Conns | Redis (ops/sec) | Ignix (ops/sec) | Ratio (Ignix/Redis) | Errors (Redis/Ignix) |")
    print("|-----------|------|-------|-----------------|-----------------|----------------------|----------------------|")

    for (size, op, conns), servers in sorted(grouped.items()):
        redis_res = servers.get('Redis')
        ignix_res = servers.get('Ignix')

        r_ops = f"{redis_res['ops_per_sec']:,.0f}" if redis_res else "N/A"
        i_ops = f"{ignix_res['ops_per_sec']:,.0f}" if ignix_res else "N/A"

        ratio = "N/A"
        if redis_res and ignix_res and redis_res['ops_per_sec'] > 0:
            r = ignix_res['ops_per_sec'] / redis_res['ops_per_sec']
            ratio = f"{r:.2f}x"
            if r > 1.1:
                ratio = f"**{ratio}**"

        errors = "/".join(str(res.get('errors', '?')) if res else "N/A" for res in (redis_res, ignix_res))

        size_str = f"{size}B"
        if size >= 1024:
            size_str = f"{size // 1024}KB"
        if size >= 1024 * 1024:
            size_str = f"{size // 1024 // 1024}MB"

        print(f"| {op} | {size_str} | {conns} | {r_ops} | {i_ops} | {ratio} | {errors} |")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="comprehensive_results")
    parser.add_argument("--target", choices=["all", "redis", "ignix"], default="all")
    parser.add_argument("--json-out", default="benchmark_results.json")
    parser.add_argument("--report-only", action="store_true")
    args = parser.parse_args()

    if args.report_only:
        generate_markdown_table(args.json_out)
        return

    # Define targets
    targets = []
    if args.target in ["all", "redis"]:
        targets.append(("localhost", 6379, "Redis"))
    if args.target in ["all", "ignix"]:
        targets.append(("localhost", 7379, "Ignix"))

    for host, port, name in targets:
        if not ping(host, port):
            print(f"❌ {name} does not answer PING on {host}:{port}")
            sys.exit(1)

    # Test cases: (warmup ops, measured ops, connections, sizes)
    cases: List[Tuple[int, int, int, List[int]]] = [
        (1000, 10000, 50, [64, 1024]),                # small values
        (500, 5000, 20, [32 * 1024, 256 * 1024]),     # medium values
        (100, 1000, 10, [2 * 1024 * 1024]),           # large values
    ]

    # Run every configuration on each server back to back.
    configs = []
    for warmup, measure, conns, sizes in cases:
        for size in sizes:
            for op in ["SET", "GET"]:
                for host, port, name in targets:
                    configs.append(BenchmarkConfig(host, port, name, warmup, measure, conns, size, op))

    results = []
    failures = 0
    for conf in configs:
        try:
            results.append(benchmark(conf))
        except Exception as e:
            failures += 1
            print(f"❌ Failed {conf.name} {conf.operation} {conf.data_size}B: {e}")

    if args.json_out:
        save_results_json(results, args.json_out)

    plot_results(results, args.out)
    print(f"\n✨ Comprehensive benchmark complete. Charts saved to {args.out}/")

    # Print summary table immediately
    generate_markdown_table(args.json_out)

    if failures or any(r.errors for r in results):
        sys.exit(1)


if __name__ == "__main__":
    main()
