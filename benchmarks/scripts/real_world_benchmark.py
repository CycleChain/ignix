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
import math
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from typing import List, Dict, Any, Tuple
import argparse

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

@dataclass
class WorkloadConfig:
    host: str
    port: int
    name: str
    num_keys: int
    num_ops: int
    connections: int
    read_ratio: float # 0.0 to 1.0
    zipf_param: float # s parameter for Zipfian distribution (s > 1)
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
    
    @property
    def total_ops(self):
        return len(self.latencies_get) + len(self.latencies_set)
        
    @property
    def duration(self):
        return self.end_time - self.start_time
        
    @property
    def throughput(self):
        return self.total_ops / self.duration if self.duration > 0 else 0

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
        f = self.sock.makefile('rb')
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

def generate_zipfian_indices(n: int, s: float, num_samples: int) -> List[int]:
    """Generate indices based on Zipfian distribution."""
    if HAS_DEPS:
        # Use numpy for efficiency
        # Zipf distribution in numpy is 1-based, so we subtract 1
        return (np.random.zipf(s, num_samples) - 1) % n
    else:
        # Fallback (slower)
        print("⚠️  Using slow fallback for Zipfian generation...")
        weights = [1.0 / math.pow(i + 1, s) for i in range(n)]
        total = sum(weights)
        probs = [w / total for w in weights]
        return random.choices(range(n), weights=probs, k=num_samples)

def generate_json_value(size: int) -> str:
    """Generate a pseudo-JSON string of approx size."""
    # Simple padding to reach size
    padding = ''.join(random.choices(string.ascii_letters, k=size - 20))
    return json.dumps({"data": padding})

def run_worker(config: WorkloadConfig, keys: List[str], key_indices: List[int]) -> Tuple[List[float], List[float], int]:
    client = RedisProtocolClient(config.host, config.port)
    if not client.connect(): return [], [], 0

    lats_get = []
    lats_set = []
    errors = 0
    
    # Pre-generate values to avoid overhead during measurement
    # We use a small pool of values
    values_pool = [generate_json_value(random.randint(config.value_size_min, config.value_size_max)) for _ in range(100)]
    
    try:
        for idx in key_indices:
            key = keys[idx]
            is_read = random.random() < config.read_ratio
            
            t0 = time.perf_counter()
            if is_read:
                ok = client.get(key)
                t1 = time.perf_counter()
                if ok: lats_get.append((t1 - t0) * 1000.0)
                else: errors += 1
            else:
                val = random.choice(values_pool)
                ok = client.set(key, val)
                t1 = time.perf_counter()
                if ok: lats_set.append((t1 - t0) * 1000.0)
                else: errors += 1
    finally:
        client.disconnect()
        
    return lats_get, lats_set, errors

def benchmark(config: WorkloadConfig) -> BenchmarkResult:
    print(f"🌍 Running Real-World Scenario: {config.name}")
    print(f"   Keys: {config.num_keys}, Ops: {config.num_ops}, Conn: {config.connections}")
    print(f"   Mix: {int(config.read_ratio*100)}% Read / {int((1-config.read_ratio)*100)}% Write")
    print(f"   Dist: Zipfian (s={config.zipf_param})")
    
    # 1. Prepare Keys
    print("   🔑 Generating keys...")
    keys = [f"user:{i}" for i in range(config.num_keys)]
    
    # 2. Pre-fill Data
    print("   📝 Pre-filling database...")
    c = RedisProtocolClient(config.host, config.port)
    if c.connect():
        # Fill a subset to save time, or all?
        # Let's fill all keys with initial data
        # Use pipeline-like batching if we had it, but here serial is fine for 10k keys
        # For 100k keys this might be slow. Let's use threads.
        def fill_batch(batch_keys):
            cl = RedisProtocolClient(config.host, config.port)
            if cl.connect():
                for k in batch_keys:
                    cl.set(k, generate_json_value(config.value_size_min))
                cl.disconnect()
        
        chunk_size = len(keys) // 10
        with ThreadPoolExecutor(max_workers=10) as ex:
            futures = []
            for i in range(0, len(keys), chunk_size):
                chunk = keys[i:i+chunk_size]
                futures.append(ex.submit(fill_batch, chunk))
            for f in as_completed(futures): f.result()
        c.disconnect()
    else:
        print("   ❌ Could not connect for pre-fill")
        return BenchmarkResult(config)

    # 3. Generate Workload Indices
    print("   🎲 Generating workload distribution...")
    # Each worker gets a slice of operations
    ops_per_worker = config.num_ops // config.connections
    
    # 4. Run Benchmark
    print(f"   🚀 Starting simulation...")
    result = BenchmarkResult(config)
    result.start_time = time.perf_counter()
    
    with ThreadPoolExecutor(max_workers=config.connections) as ex:
        futures = []
        for _ in range(config.connections):
            # Generate indices for this worker
            indices = generate_zipfian_indices(config.num_keys, config.zipf_param, ops_per_worker)
            futures.append(ex.submit(run_worker, config, keys, indices))
            
        for f in as_completed(futures):
            lg, ls, err = f.result()
            result.latencies_get.extend(lg)
            result.latencies_set.extend(ls)
            result.errors += err
            
    result.end_time = time.perf_counter()
    
    print(f"   ✅ Done! Throughput: {result.throughput:.1f} ops/sec")
    print(f"      GET Avg: {statistics.mean(result.latencies_get) if result.latencies_get else 0:.3f}ms")
    print(f"      SET Avg: {statistics.mean(result.latencies_set) if result.latencies_set else 0:.3f}ms")
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
    sns.barplot(data=df, x="Server", y="Throughput", palette="viridis")
    plt.title("Real-World Scenario Throughput (Session Store) - Higher is Better")
    plt.ylabel("Requests / Second")
    plt.savefig(f"{output_dir}/real_world_throughput.png")
    plt.close()
    
    # 2. Latency Distribution (Combined)
    plt.figure(figsize=(12, 6))
    lat_data = []
    for r in results:
        # Sample GET latencies
        gets = r.latencies_get if len(r.latencies_get) < 5000 else random.sample(r.latencies_get, 5000)
        for l in gets: lat_data.append({"Server": r.config.name, "Type": "GET", "Latency": l})
        
        # Sample SET latencies
        sets = r.latencies_set if len(r.latencies_set) < 5000 else random.sample(r.latencies_set, 5000)
        for l in sets: lat_data.append({"Server": r.config.name, "Type": "SET", "Latency": l})
            
    df_lat = pd.DataFrame(lat_data)
    sns.boxplot(data=df_lat, x="Type", y="Latency", hue="Server", palette="viridis", showfliers=False)
    plt.title("Latency Distribution by Operation Type (Lower is Better)")
    plt.ylabel("Latency (ms)")
    plt.savefig(f"{output_dir}/real_world_latency.png")
    plt.close()

def save_results_json(results: List[BenchmarkResult], filename: str):
    data = []
    if os.path.exists(filename):
        try:
            with open(filename, 'r') as f:
                data = json.load(f)
        except:
            pass
            
    for r in results:
        # Remove existing entry for this server if present to avoid duplicates
        data = [d for d in data if d['name'] != r.config.name]
        
        data.append({
            "name": r.config.name,
            "throughput": r.throughput,
            "avg_latency_get": statistics.mean(r.latencies_get) if r.latencies_get else 0,
            "avg_latency_set": statistics.mean(r.latencies_set) if r.latencies_set else 0,
            "p99_latency_get": statistics.quantiles(r.latencies_get, n=100)[98] if len(r.latencies_get) >= 100 else 0,
            "p99_latency_set": statistics.quantiles(r.latencies_set, n=100)[98] if len(r.latencies_set) >= 100 else 0
        })
        
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

    # Throughput
    r_t = redis_res['throughput']
    i_t = ignix_res['throughput']
    ratio_t = i_t / r_t if r_t > 0 else 0
    ratio_str = f"**{ratio_t:.2f}x**" if ratio_t > 1.1 else f"{ratio_t:.2f}x"
    print(f"| Throughput | {r_t:,.0f} ops/sec | {i_t:,.0f} ops/sec | {ratio_str} |")
    
    # Avg Latency (Combined approximation)
    r_lat = (redis_res['avg_latency_get'] + redis_res['avg_latency_set']) / 2
    i_lat = (ignix_res['avg_latency_get'] + ignix_res['avg_latency_set']) / 2
    ratio_l = i_lat / r_lat if r_lat > 0 else 0
    # Lower is better for latency
    ratio_l_str = f"**{ratio_l:.2f}x**" if ratio_l < 0.9 else f"{ratio_l:.2f}x"
    print(f"| Avg Latency | {r_lat:.2f} ms | {i_lat:.2f} ms | {ratio_l_str} |")

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
    # 100k keys, 100k ops, 50 conns, 80% read, Zipf 1.2, 1KB-2KB values
    common_config = {
        "num_keys": 10000, # Reduced to 10k for faster test
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
    
    results = []
    for conf in configs:
        try:
            res = benchmark(conf)
            results.append(res)
        except Exception as e:
            print(f"❌ Failed {conf.name}: {e}")
            
    if args.json_out:
        save_results_json(results, args.json_out)
            
    plot_comparison(results, args.out)
    print(f"\n✨ Real-world benchmark complete. Charts saved to {args.out}/")
    
    generate_markdown_table(args.json_out)

if __name__ == "__main__":
    main()
