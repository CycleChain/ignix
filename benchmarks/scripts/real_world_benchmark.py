#!/usr/bin/env python3
"""Session-store style mixed workload against Redis and Ignix.

80% GET / 20% SET over 10,000 pre-filled keys with a Zipf access pattern
(truncated to the key range) and 1-2 KB JSON-like values. Every GET must
return a stored value and every SET must answer OK; failures are counted,
printed and saved. Values, key indices and the read/write mix are generated
before the clock starts, and throughput is measured over the request loops
only.
"""

import argparse
import json
import math
import os
import random
import statistics
import string
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import List

from resp_client import ProtocolError, RespClient, RespError, encode_command, ping

try:
    import matplotlib.pyplot as plt
    import seaborn as sns
    import numpy as np
    import pandas as pd
    HAS_DEPS = True
except ImportError:
    HAS_DEPS = False
    print("⚠️  numpy, pandas, matplotlib, or seaborn not found.")
    print("   Installation: pip install numpy pandas matplotlib seaborn")

MAX_ERROR_SAMPLES = 5
VALUE_PREFIX = b'{"data": "'


@dataclass
class WorkloadConfig:
    host: str
    port: int
    name: str
    num_keys: int
    num_ops: int
    connections: int
    read_ratio: float  # 0.0 to 1.0
    zipf_param: float  # s parameter for Zipfian distribution (s > 1)
    value_size_min: int
    value_size_max: int


@dataclass
class BenchmarkResult:
    config: WorkloadConfig
    latencies_get: List[float] = field(default_factory=list)
    latencies_set: List[float] = field(default_factory=list)
    start_time: float = 0.0
    end_time: float = 0.0
    errors: int = 0
    error_samples: List[str] = field(default_factory=list)

    @property
    def total_ops(self):
        return len(self.latencies_get) + len(self.latencies_set)

    @property
    def duration(self):
        return self.end_time - self.start_time

    @property
    def throughput(self):
        return self.total_ops / self.duration if self.duration > 0 else 0

    @property
    def avg_latency(self):
        """Mean over all requests, so it reflects the actual read/write mix."""
        latencies = self.latencies_get + self.latencies_set
        return statistics.mean(latencies) if latencies else 0


@dataclass
class WorkerPlan:
    requests: List[bytes]
    is_get: List[bool]


@dataclass
class WorkerResult:
    latencies_get: List[float] = field(default_factory=list)
    latencies_set: List[float] = field(default_factory=list)
    errors: int = 0
    error_samples: List[str] = field(default_factory=list)
    start: float = 0.0
    end: float = 0.0


def generate_zipfian_indices(n: int, s: float, num_samples: int) -> List[int]:
    """Key indices in [0, n) following a Zipf distribution truncated to n."""
    if HAS_DEPS:
        indices: List[int] = []
        while len(indices) < num_samples:
            draw = np.random.zipf(s, 2 * (num_samples - len(indices)))
            indices.extend((draw[draw <= n] - 1).tolist())
        return indices[:num_samples]
    weights = [1.0 / math.pow(i + 1, s) for i in range(n)]
    return random.choices(range(n), weights=weights, k=num_samples)


def generate_json_value(size: int) -> bytes:
    """A pseudo-JSON value of roughly `size` bytes."""
    padding = ''.join(random.choices(string.ascii_letters, k=max(size - 20, 0)))
    return json.dumps({"data": padding}).encode()


def plan_worker(config: WorkloadConfig, keys: List[bytes], ops: int) -> WorkerPlan:
    """Encode all requests of one worker ahead of the measurement."""
    values = [generate_json_value(random.randint(config.value_size_min, config.value_size_max))
              for _ in range(100)]
    plan = WorkerPlan([], [])
    for idx in generate_zipfian_indices(config.num_keys, config.zipf_param, ops):
        if random.random() < config.read_ratio:
            plan.requests.append(encode_command("GET", keys[idx]))
            plan.is_get.append(True)
        else:
            plan.requests.append(encode_command("SET", keys[idx], random.choice(values)))
            plan.is_get.append(False)
    return plan


def run_worker(config: WorkloadConfig, plan: WorkerPlan) -> WorkerResult:
    result = WorkerResult()
    client = RespClient(config.host, config.port)
    try:
        client.connect()
    except OSError as e:
        result.errors = len(plan.requests)
        result.error_samples.append(f"connect: {e}")
        return result

    try:
        result.start = time.perf_counter()
        for i, (request, is_get) in enumerate(zip(plan.requests, plan.is_get)):
            t0 = time.perf_counter()
            try:
                reply = client.request(request)
            except RespError as e:
                reply = e
            except (OSError, ProtocolError, ValueError) as e:
                # The connection is unusable: count the rest as failed.
                result.errors += len(plan.requests) - i
                result.error_samples.append(f"{type(e).__name__}: {e}")
                break
            t1 = time.perf_counter()
            if is_get:
                # Every key was pre-filled with a JSON value.
                ok = isinstance(reply, bytes) and reply.startswith(VALUE_PREFIX)
            else:
                ok = reply == b"OK"
            if ok:
                (result.latencies_get if is_get else result.latencies_set).append((t1 - t0) * 1000.0)
            else:
                result.errors += 1
                if len(result.error_samples) < MAX_ERROR_SAMPLES:
                    op = "GET" if is_get else "SET"
                    result.error_samples.append(f"{op} got unexpected reply: {str(reply)[:80]}")
        result.end = time.perf_counter()
    finally:
        client.close()
    return result


def prefill(config: WorkloadConfig, keys: List[bytes]) -> None:
    """Write every key once so all GETs hit; fail loudly on any error."""
    def fill_batch(batch_keys: List[bytes]) -> None:
        with RespClient(config.host, config.port, timeout=30.0) as client:
            for key in batch_keys:
                reply = client.execute("SET", key, generate_json_value(config.value_size_min))
                if reply != b"OK":
                    raise RuntimeError(f"pre-fill SET {key!r} answered {reply!r}")

    chunk_size = max(len(keys) // 10, 1)
    with ThreadPoolExecutor(max_workers=10) as ex:
        futures = [ex.submit(fill_batch, keys[i:i + chunk_size])
                   for i in range(0, len(keys), chunk_size)]
        for f in futures:
            f.result()


def benchmark(config: WorkloadConfig) -> BenchmarkResult:
    print(f"🌍 Running Real-World Scenario: {config.name}")
    print(f"   Keys: {config.num_keys}, Ops: {config.num_ops}, Conn: {config.connections}")
    print(f"   Mix: {round(config.read_ratio * 100)}% Read / {round((1 - config.read_ratio) * 100)}% Write")
    print(f"   Dist: Zipfian (s={config.zipf_param})")

    keys = [f"user:{i}".encode() for i in range(config.num_keys)]

    print("   📝 Pre-filling database...")
    prefill(config, keys)

    print("   🎲 Generating workload...")
    ops_per_worker = config.num_ops // config.connections
    plans = [plan_worker(config, keys, ops_per_worker) for _ in range(config.connections)]

    print("   🚀 Starting simulation...")
    result = BenchmarkResult(config)
    with ThreadPoolExecutor(max_workers=config.connections) as ex:
        workers = list(ex.map(lambda plan: run_worker(config, plan), plans))

    for w in workers:
        result.latencies_get.extend(w.latencies_get)
        result.latencies_set.extend(w.latencies_set)
        result.errors += w.errors
        result.error_samples.extend(w.error_samples[:MAX_ERROR_SAMPLES - len(result.error_samples)])
    timed = [w for w in workers if w.end > 0]
    if timed:
        result.start_time = min(w.start for w in timed)
        result.end_time = max(w.end for w in timed)

    print(f"   ✅ Done! Throughput: {result.throughput:.1f} ops/sec, errors: {result.errors}")
    print(f"      GET Avg: {statistics.mean(result.latencies_get) if result.latencies_get else 0:.3f}ms")
    print(f"      SET Avg: {statistics.mean(result.latencies_set) if result.latencies_set else 0:.3f}ms")
    for sample in result.error_samples:
        print(f"      ⚠️  {sample}")
    print("-" * 60)
    return result


def plot_comparison(results: List[BenchmarkResult], output_dir: str):
    if not HAS_DEPS:
        print("⚠️  Skipping plot generation: numpy/pandas/matplotlib/seaborn not found.")
        return

    if not results:
        print("⚠️  Skipping plot generation: No benchmark results to plot.")
        return
    os.makedirs(output_dir, exist_ok=True)
    sns.set_theme(style="whitegrid")

    # 1. Throughput
    plt.figure(figsize=(10, 6))
    data = []
    for r in results:
        data.append({"Server": r.config.name, "Throughput": r.throughput})

    df = pd.DataFrame(data)
    sns.barplot(data=df, x="Server", y="Throughput", hue="Server", palette="viridis")
    plt.title("Real-World Scenario Throughput (Session Store) - Higher is Better")
    plt.ylabel("Requests / Second")
    plt.savefig(f"{output_dir}/real_world_throughput.png")
    plt.close()

    # 2. Latency Distribution (Combined)
    plt.figure(figsize=(12, 6))
    lat_data = []
    for r in results:
        gets = r.latencies_get if len(r.latencies_get) < 5000 else random.sample(r.latencies_get, 5000)
        for l in gets:
            lat_data.append({"Server": r.config.name, "Type": "GET", "Latency": l})
        sets = r.latencies_set if len(r.latencies_set) < 5000 else random.sample(r.latencies_set, 5000)
        for l in sets:
            lat_data.append({"Server": r.config.name, "Type": "SET", "Latency": l})

    df_lat = pd.DataFrame(lat_data)
    sns.boxplot(data=df_lat, x="Type", y="Latency", hue="Server", palette="viridis", showfliers=False)
    plt.title("Latency Distribution by Operation Type (Lower is Better)")
    plt.ylabel("Latency (ms)")
    plt.savefig(f"{output_dir}/real_world_latency.png")
    plt.close()


def p99(latencies: List[float]) -> float:
    return statistics.quantiles(latencies, n=100)[98] if len(latencies) >= 100 else 0


def save_results_json(results: List[BenchmarkResult], filename: str):
    data = []
    if os.path.exists(filename):
        try:
            with open(filename, 'r') as f:
                data = json.load(f)
        except (OSError, ValueError):
            data = []

    measured_at = time.strftime("%Y-%m-%dT%H:%M:%S")
    for r in results:
        # Replace an earlier entry for this server
        data = [d for d in data if d.get('name') != r.config.name]
        data.append({
            "name": r.config.name,
            "throughput": r.throughput,
            "avg_latency": r.avg_latency,
            "avg_latency_get": statistics.mean(r.latencies_get) if r.latencies_get else 0,
            "avg_latency_set": statistics.mean(r.latencies_set) if r.latencies_set else 0,
            "p99_latency_get": p99(r.latencies_get),
            "p99_latency_set": p99(r.latencies_set),
            "successful_ops": r.total_ops,
            "errors": r.errors,
            "measured_at": measured_at,
        })

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

    print("\n### Real-World Scenario Results\n")
    print("| Metric | Redis | Ignix | Ratio (Ignix/Redis) |")
    print("|--------|-------|-------|----------------------|")

    redis_res = next((d for d in data if d['name'] == 'Redis'), None)
    ignix_res = next((d for d in data if d['name'] == 'Ignix'), None)

    if not redis_res or not ignix_res:
        print("Waiting for both results...")
        return

    r_t = redis_res['throughput']
    i_t = ignix_res['throughput']
    ratio_t = i_t / r_t if r_t > 0 else 0
    ratio_str = f"**{ratio_t:.2f}x**" if ratio_t > 1.1 else f"{ratio_t:.2f}x"
    print(f"| Throughput | {r_t:,.0f} ops/sec | {i_t:,.0f} ops/sec | {ratio_str} |")

    # Weighted by the actual number of GETs and SETs
    r_lat = redis_res.get('avg_latency', 0)
    i_lat = ignix_res.get('avg_latency', 0)
    ratio_l = i_lat / r_lat if r_lat > 0 else 0
    # Lower is better for latency
    ratio_l_str = f"**{ratio_l:.2f}x**" if 0 < ratio_l < 0.9 else f"{ratio_l:.2f}x"
    print(f"| Avg Latency | {r_lat:.2f} ms | {i_lat:.2f} ms | {ratio_l_str} |")
    print(f"| Errors | {redis_res.get('errors', '?')} | {ignix_res.get('errors', '?')} | |")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="real_world_results")
    parser.add_argument("--target", choices=["all", "redis", "ignix"], default="all")
    parser.add_argument("--json-out", default="real_world_results.json")
    parser.add_argument("--report-only", action="store_true")
    args = parser.parse_args()

    if args.report_only:
        generate_markdown_table(args.json_out)
        return

    # Scenario: Session Store
    # 10k keys, 50k ops, 50 conns, 80% read, Zipf 1.2, 1KB-2KB values
    common_config = {
        "num_keys": 10000,
        "num_ops": 50000,
        "connections": 50,
        "read_ratio": 0.8,
        "zipf_param": 1.2,
        "value_size_min": 1024,
        "value_size_max": 2048
    }

    configs = []
    if args.target in ["all", "redis"]:
        configs.append(WorkloadConfig(host="localhost", port=6379, name="Redis", **common_config))
    if args.target in ["all", "ignix"]:
        configs.append(WorkloadConfig(host="localhost", port=7379, name="Ignix", **common_config))

    for conf in configs:
        if not ping(conf.host, conf.port):
            print(f"❌ {conf.name} does not answer PING on {conf.host}:{conf.port}")
            sys.exit(1)

    results = []
    failures = 0
    for conf in configs:
        try:
            results.append(benchmark(conf))
        except Exception as e:
            failures += 1
            print(f"❌ Failed {conf.name}: {e}")

    if args.json_out:
        save_results_json(results, args.json_out)

    plot_comparison(results, args.out)
    print(f"\n✨ Real-world benchmark complete. Charts saved to {args.out}/")

    generate_markdown_table(args.json_out)

    if failures or any(r.errors for r in results):
        sys.exit(1)


if __name__ == "__main__":
    main()
