#!/usr/bin/env python3

import socket
import time
import threading
import statistics
import json
import os
import sys
import random
import string
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, asdict, field
from typing import List, Dict, Any, Tuple
import argparse

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
    throughput_samples: List[Tuple[float, int]] = field(default_factory=list) # (timestamp, ops_count)

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
        if not self.latencies: return 0
        return statistics.quantiles(self.latencies, n=1000)[int(p*10)-1] if len(self.latencies) >= 1000 else statistics.quantiles(self.latencies, n=100)[int(p)-1]

class RedisProtocolClient:
    def __init__(self, host: str, port: int, timeout: float = 5.0):
        self.host = host
        self.port = port
        self.timeout = timeout
        self.sock = None

    def connect(self) -> bool:
        try:
            self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            self.sock.settimeout(self.timeout)
            self.sock.connect((self.host, self.port))
            return True
        except Exception as e:
            # print(f"Connection error: {e}")
            return False

    def disconnect(self):
        if self.sock:
            try:
                self.sock.close()
            except:
                pass
            self.sock = None

    def _send_command(self, *args) -> bytes:
        if not self.sock: raise Exception("No connection")
        
        resp = f"*{len(args)}\r\n"
        for arg in args:
            s = str(arg).encode('utf-8')
            resp += f"${len(s)}\r\n"
            resp += s.decode('utf-8') + "\r\n"
        
        self.sock.sendall(resp.encode('utf-8'))
        return self._read_response()

    def _read_response(self) -> bytes:
        f = self.sock.makefile('rb') # Simplified reading for benchmark script
        line = f.readline()
        if not line: raise Exception("Connection closed")
        
        if line.startswith(b'+'): return line.strip()
        elif line.startswith(b'-'): raise Exception(line.strip().decode())
        elif line.startswith(b':'): return line.strip()
        elif line.startswith(b'$'):
            length = int(line[1:])
            if length == -1: return None
            data = f.read(length)
            f.read(2) # CRLF
            return data
        elif line.startswith(b'*'):
            count = int(line[1:])
            res = []
            for _ in range(count):
                res.append(self._read_response()) # Recursive for arrays (not used in simple bench)
            return res
        return line

    def set(self, key: str, value: str) -> bool:
        try:
            return self._send_command("SET", key, value) == b'+OK'
        except:
            return False

    def get(self, key: str) -> bool:
        try:
            self._send_command("GET", key)
            return True
        except:
            return False

def generate_data(size: int) -> str:
    return ''.join(random.choices(string.ascii_letters + string.digits, k=size))

def run_worker(config: BenchmarkConfig, keys: List[str], values: List[str]) -> Tuple[List[float], int]:
    client = RedisProtocolClient(config.host, config.port)
    if not client.connect(): return [], config.measure_ops // config.connections

    latencies = []
    errors = 0
    ops_per_worker = config.measure_ops // config.connections
    
    try:
        for i in range(ops_per_worker):
            k = keys[i % len(keys)]
            v = values[i % len(values)]
            
            t0 = time.perf_counter()
            if config.operation == "SET":
                ok = client.set(k, v)
            else:
                ok = client.get(k)
            t1 = time.perf_counter()
            
            if ok:
                latencies.append((t1 - t0) * 1000.0) # ms
            else:
                errors += 1
    finally:
        client.disconnect()
        
    return latencies, errors

def benchmark(config: BenchmarkConfig) -> BenchmarkResult:
    print(f"🚀 Benchmarking {config.name} ({config.host}:{config.port})")
    print(f"   Op: {config.operation}, Size: {config.data_size}B, Conn: {config.connections}")
    
    # Prepare data
    keys = [f"key_{i}" for i in range(1000)]
    val = generate_data(config.data_size)
    values = [val] * 1000 # Reuse same value string to save memory
    
    # Pre-fill for GET
    if config.operation == "GET":
        print("   📝 Pre-filling data...")
        c = RedisProtocolClient(config.host, config.port)
        if c.connect():
            for k in keys: c.set(k, val)
            c.disconnect()
        else:
            print("   ❌ Could not connect for pre-fill")
            return BenchmarkResult(config)

    # Warmup
    if config.warmup_ops > 0:
        print(f"   🔥 Warming up ({config.warmup_ops} ops)...")
        with ThreadPoolExecutor(max_workers=config.connections) as ex:
            futures = []
            warmup_per_worker = config.warmup_ops // config.connections
            for _ in range(config.connections):
                futures.append(ex.submit(run_worker, 
                    BenchmarkConfig(config.host, config.port, config.name, 0, config.warmup_ops, config.connections, config.data_size, config.operation),
                    keys, values))
            for f in as_completed(futures): f.result()

    # Measurement
    print(f"   ⏱️  Measuring ({config.measure_ops} ops)...")
    result = BenchmarkResult(config)
    result.start_time = time.perf_counter()
    
    with ThreadPoolExecutor(max_workers=config.connections) as ex:
        futures = []
        for _ in range(config.connections):
            futures.append(ex.submit(run_worker, config, keys, values))
            
        for f in as_completed(futures):
            lats, errs = f.result()
            result.latencies.extend(lats)
            result.errors += errs
            
    result.end_time = time.perf_counter()
    
    print(f"   ✅ Done! {result.ops_per_sec:.1f} ops/sec, Avg Lat: {result.avg_latency:.3f}ms")
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
    plt.figure(figsize=(12, 6))
    percentiles = [50, 90, 95, 99, 99.9]
    p_data = []
    
    for r in results:
        if not r.latencies: continue
        sorted_lats = sorted(r.latencies)
        n = len(sorted_lats)
        for p in percentiles:
            idx = int(n * (p/100.0)) - 1
            val = sorted_lats[idx]
            p_data.append({
                "Server": r.config.name,
                "Percentile": str(p),
                "Latency (ms)": val,
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

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="comprehensive_results")
    args = parser.parse_args()
    
    configs = []
    # Test Cases
    # Small sizes: High ops count
    for size in [64, 1024]:
        for op in ["SET", "GET"]:
            configs.append(BenchmarkConfig("localhost", 6379, "Redis", 1000, 10000, 50, size, op))
            configs.append(BenchmarkConfig("localhost", 7379, "Ignix", 1000, 10000, 50, size, op))

    # Medium sizes: Moderate ops count
    for size in [32 * 1024, 256 * 1024]: # 32KB, 256KB
        for op in ["SET", "GET"]:
            configs.append(BenchmarkConfig("localhost", 6379, "Redis", 500, 5000, 20, size, op))
            configs.append(BenchmarkConfig("localhost", 7379, "Ignix", 500, 5000, 20, size, op))

    # Large sizes: Low ops count
    for size in [2 * 1024 * 1024]: # 2MB
        for op in ["SET", "GET"]:
            configs.append(BenchmarkConfig("localhost", 6379, "Redis", 100, 1000, 10, size, op))
            configs.append(BenchmarkConfig("localhost", 7379, "Ignix", 100, 1000, 10, size, op))

    results = []
    for conf in configs:
        try:
            res = benchmark(conf)
            results.append(res)
        except Exception as e:
            print(f"❌ Failed: {e}")

    plot_results(results, args.out)
    print(f"\n✨ Comprehensive benchmark complete. Charts saved to {args.out}/")

if __name__ == "__main__":
    main()
