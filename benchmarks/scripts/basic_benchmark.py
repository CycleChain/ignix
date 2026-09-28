#!/usr/bin/env python3
"""Basic SET/GET comparison of Redis and Ignix over several value sizes and
connection counts.

Every reply is checked (SET must answer OK, GET must return the exact value
that was written); anything else counts as an error. Throughput is measured
over the request loops only, so connecting is not timed.
"""

import argparse
import json
import os
import statistics
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, asdict
from typing import List, Optional

from resp_client import ProtocolError, RespClient, RespError, encode_command, ping

try:
    import matplotlib.pyplot as plt
    import seaborn as sns
    HAS_PLOTTING = True
except ImportError:
    HAS_PLOTTING = False
    print("⚠️  matplotlib and seaborn not found. Graphs cannot be created.")
    print("   Installation: pip install matplotlib seaborn")

KEYS = 1000


@dataclass
class BenchmarkResult:
    """Data structure for benchmark results"""
    server_name: str
    operation: str  # 'SET' or 'GET'
    data_size: int  # bytes
    concurrent_connections: int
    total_operations: int
    total_time: float  # seconds
    operations_per_second: float
    avg_latency_ms: float
    min_latency_ms: float
    max_latency_ms: float
    p95_latency_ms: float
    p99_latency_ms: float
    error_count: int
    success_rate: float


def generate_test_data(size: int) -> bytes:
    """Generate test data"""
    if size <= 10:
        return b"x" * size
    # Use pattern for more realistic data
    pattern = b"abcdefghijklmnopqrstuvwxyz0123456789"
    repeats, remainder = divmod(size, len(pattern))
    return pattern * repeats + pattern[:remainder]


def run_operation_batch(host: str, port: int, requests: List[bytes],
                        expected: List[bytes], batch_size: int):
    """Run `batch_size` requests on one connection and check every reply.

    Returns (latencies, errors, loop start, loop end)."""
    latencies: List[float] = []
    client = RespClient(host, port)
    try:
        client.connect()
    except OSError:
        return latencies, batch_size, 0.0, 0.0

    errors = 0
    start = time.perf_counter()
    try:
        for i in range(batch_size):
            index = i % len(requests)
            t0 = time.perf_counter()
            try:
                reply = client.request(requests[index])
            except RespError:
                reply = None
            except (OSError, ProtocolError, ValueError):
                # The connection is unusable: count the rest as failed.
                errors += batch_size - i
                break
            t1 = time.perf_counter()
            if reply == expected[index]:
                latencies.append((t1 - t0) * 1000)
            else:
                errors += 1
    finally:
        end = time.perf_counter()
        client.close()
    return latencies, errors, start, end


def benchmark_server(host: str, port: int, server_name: str,
                     operation: str, data_size: int,
                     concurrent_connections: int, operations_per_connection: int) -> BenchmarkResult:
    """Run benchmark for a single server"""

    print(f"🔄 {server_name} - {operation} benchmark starting...")
    print(f"   Data size: {data_size} bytes")
    print(f"   Concurrent connections: {concurrent_connections}")
    print(f"   Total operations: {concurrent_connections * operations_per_connection}")

    # Prepare test data and encode the requests before timing
    keys = [f"benchmark_key_{i}".encode() for i in range(KEYS)]
    value = generate_test_data(data_size)

    if operation == "GET":
        print("   📝 Preparing data for GET test...")
        with RespClient(host, port, timeout=30.0) as setup_client:
            for key in keys:
                reply = setup_client.execute("SET", key, value)
                if reply != b"OK":
                    raise RuntimeError(f"pre-fill SET {key!r} answered {reply!r}")
        requests = [encode_command("GET", k) for k in keys]
        expected = [value] * KEYS
    else:
        requests = [encode_command("SET", k, value) for k in keys]
        expected = [b"OK"] * KEYS

    with ThreadPoolExecutor(max_workers=concurrent_connections) as executor:
        futures = [executor.submit(run_operation_batch, host, port, requests, expected,
                                   operations_per_connection)
                   for _ in range(concurrent_connections)]
        batches = [f.result() for f in futures]

    all_latencies = [lat for batch in batches for lat in batch[0]]
    total_errors = sum(batch[1] for batch in batches)
    timed = [batch for batch in batches if batch[3] > 0]
    total_time = (max(b[3] for b in timed) - min(b[2] for b in timed)) if timed else 0.0
    total_operations = concurrent_connections * operations_per_connection
    successful_operations = len(all_latencies)

    # Calculate statistics
    if len(all_latencies) >= 2:
        avg_latency = statistics.mean(all_latencies)
        min_latency = min(all_latencies)
        max_latency = max(all_latencies)
        p95_latency = statistics.quantiles(all_latencies, n=20)[18]  # 95th percentile
        p99_latency = statistics.quantiles(all_latencies, n=100)[98]  # 99th percentile
    else:
        avg_latency = min_latency = max_latency = p95_latency = p99_latency = 0

    ops_per_second = successful_operations / total_time if total_time > 0 else 0
    success_rate = successful_operations / total_operations if total_operations > 0 else 0

    result = BenchmarkResult(
        server_name=server_name,
        operation=operation,
        data_size=data_size,
        concurrent_connections=concurrent_connections,
        total_operations=total_operations,
        total_time=total_time,
        operations_per_second=ops_per_second,
        avg_latency_ms=avg_latency,
        min_latency_ms=min_latency,
        max_latency_ms=max_latency,
        p95_latency_ms=p95_latency,
        p99_latency_ms=p99_latency,
        error_count=total_errors,
        success_rate=success_rate
    )

    print(f"✅ {server_name} - {operation} completed!")
    print(f"   Operations/second: {ops_per_second:.1f}")
    print(f"   Average latency: {avg_latency:.2f} ms")
    print(f"   Success rate: {success_rate*100:.1f}% ({total_errors} errors)")
    print()

    return result


def find(results: List[BenchmarkResult], server: str, operation: str, size: int,
         connections: int) -> Optional[BenchmarkResult]:
    return next((r for r in results
                 if r.server_name == server and r.operation == operation
                 and r.data_size == size and r.concurrent_connections == connections), None)


def print_results_table(results: List[BenchmarkResult]):
    """Print results in table format"""
    print("\n" + "="*120)
    print("🏆 BENCHMARK RESULTS")
    print("="*120)

    # Header
    print(f"{'Server':<10} {'Operation':<9} {'Data Size':<10} {'Connections':<11} "
          f"{'Ops/sec':<10} {'Avg Lat(ms)':<12} {'P95(ms)':<9} {'P99(ms)':<9} {'Success%':<9}")
    print("-"*120)

    for result in results:
        print(f"{result.server_name:<10} {result.operation:<9} {result.data_size:<10} "
              f"{result.concurrent_connections:<11} {result.operations_per_second:<10.1f} "
              f"{result.avg_latency_ms:<12.2f} {result.p95_latency_ms:<9.2f} "
              f"{result.p99_latency_ms:<9.2f} {result.success_rate*100:<9.1f}")

    print("-"*120)

    # Comparison for every (operation, size, connections) measured on both servers
    print("\n🔍 COMPARISON SUMMARY:")
    print("-"*50)

    combos = sorted({(r.operation, r.data_size, r.concurrent_connections) for r in results})
    for operation, data_size, connections in combos:
        redis_result = find(results, "Redis", operation, data_size, connections)
        ignix_result = find(results, "Ignix", operation, data_size, connections)
        if not (redis_result and ignix_result):
            continue
        print(f"\n{operation} ({data_size} bytes, {connections} connections):")
        if redis_result.operations_per_second > 0:
            ops_ratio = ignix_result.operations_per_second / redis_result.operations_per_second
            print(f"  Throughput: Ignix {ops_ratio:.2f}x Redis")
        else:
            print("  Throughput: n/a (Redis had no successful operations)")
        if redis_result.avg_latency_ms > 0 and ignix_result.avg_latency_ms > 0:
            lat_ratio = redis_result.avg_latency_ms / ignix_result.avg_latency_ms
            print(f"  Latency: Ignix {lat_ratio:.2f}x better" if lat_ratio > 1
                  else f"  Latency: Redis {1/lat_ratio:.2f}x better")
        else:
            print("  Latency: n/a")


def create_visualizations(results: List[BenchmarkResult], output_dir: str = "benchmark_results"):
    """Create visualization charts, one set per connection count"""
    if not HAS_PLOTTING:
        print("⚠️  Matplotlib not found, cannot create charts.")
        return

    os.makedirs(output_dir, exist_ok=True)

    # Style settings
    plt.style.use('seaborn-v0_8')
    sns.set_palette("husl")

    operations = ['SET', 'GET']
    for connections in sorted({r.concurrent_connections for r in results}):
        subset = [r for r in results if r.concurrent_connections == connections]

        fig, axes = plt.subplots(2, 2, figsize=(15, 12))
        fig.suptitle(f'Redis vs Ignix Performance Comparison ({connections} connections)',
                     fontsize=16, fontweight='bold')

        for row, (metric, title, ylabel) in enumerate([
            ('operations_per_second', 'Operations per Second', 'Operations/second'),
            ('avg_latency_ms', 'Average Latency', 'Latency (ms)'),
        ]):
            for i, operation in enumerate(operations):
                data_sizes = sorted({r.data_size for r in subset if r.operation == operation})
                redis_vals = [getattr(find(subset, "Redis", operation, s, connections), metric, 0) for s in data_sizes]
                ignix_vals = [getattr(find(subset, "Ignix", operation, s, connections), metric, 0) for s in data_sizes]

                x = range(len(data_sizes))
                width = 0.35
                ax = axes[row, i]
                ax.bar([xi - width/2 for xi in x], redis_vals, width, label='Redis', alpha=0.8)
                ax.bar([xi + width/2 for xi in x], ignix_vals, width, label='Ignix', alpha=0.8)
                ax.set_title(f'{operation} {title}')
                ax.set_xlabel('Data Size (bytes)')
                ax.set_ylabel(ylabel)
                ax.set_xticks(list(x))
                ax.set_xticklabels([str(s) for s in data_sizes])
                ax.legend()
                ax.grid(True, alpha=0.3)

        plt.tight_layout()
        plt.savefig(f"{output_dir}/redis_vs_ignix_comparison_c{connections}.png", dpi=150, bbox_inches='tight')
        plt.close()

        # Performance ratio chart
        ratios, labels = [], []
        for operation in operations:
            for size in sorted({r.data_size for r in subset if r.operation == operation}):
                redis_result = find(subset, "Redis", operation, size, connections)
                ignix_result = find(subset, "Ignix", operation, size, connections)
                if redis_result and ignix_result and redis_result.operations_per_second > 0:
                    ratios.append(ignix_result.operations_per_second / redis_result.operations_per_second)
                    labels.append(f"{operation}\n{size}B")

        fig, ax = plt.subplots(1, 1, figsize=(12, 8))
        bars = ax.bar(labels, ratios, color=['green' if r > 1 else 'red' for r in ratios], alpha=0.7)
        ax.axhline(y=1, color='black', linestyle='--', alpha=0.5, label='Equal Performance')
        ax.set_title(f'Ignix vs Redis Performance Ratio, {connections} connections\n(>1 means Ignix is faster)',
                     fontweight='bold')
        ax.set_ylabel('Performance Ratio (Ignix/Redis)')
        ax.set_xlabel('Test Configuration')
        ax.grid(True, alpha=0.3)
        for bar, ratio in zip(bars, ratios):
            ax.text(bar.get_x() + bar.get_width()/2., bar.get_height() + 0.01,
                    f'{ratio:.2f}x', ha='center', va='bottom', fontweight='bold')
        plt.tight_layout()
        plt.savefig(f"{output_dir}/performance_ratio_c{connections}.png", dpi=150, bbox_inches='tight')
        plt.close()

    print(f"📊 Charts created: {output_dir}/")


def save_results_json(results: List[BenchmarkResult], filename: str = "benchmark_results.json"):
    """Save results in JSON format"""
    results_dict = {
        "timestamp": time.strftime("%Y-%m-%d %H:%M:%S"),
        "results": [asdict(result) for result in results]
    }

    with open(filename, 'w', encoding='utf-8') as f:
        json.dump(results_dict, f, indent=2, ensure_ascii=False)

    print(f"💾 Results saved: {filename}")


def check_prerequisites():
    """Check prerequisites"""
    print("🔍 Checking prerequisites...")

    redis_available = ping("localhost", 6379)
    print(f"{'✅' if redis_available else '❌'} Redis (localhost:6379): {'Accessible' if redis_available else 'Not accessible'}")

    ignix_available = ping("localhost", 7379)
    print(f"{'✅' if ignix_available else '❌'} Ignix (localhost:7379): {'Accessible' if ignix_available else 'Not accessible'}")

    if not redis_available:
        print("\n⚠️  Redis server is not running!")
        print("   To start Redis: redis-server")

    if not ignix_available:
        print("\n⚠️  Ignix server is not running!")
        print("   To start Ignix: cargo run --release")

    if not (redis_available and ignix_available):
        print("\n❌ Both servers must be running!")
        return False

    print("✅ All prerequisites met!\n")
    return True


def main():
    """Main benchmark function"""
    parser = argparse.ArgumentParser(description="Redis vs Ignix Performance Benchmark")
    parser.add_argument("--data-sizes", nargs='+', type=int, default=[64, 256, 1024, 4096],
                        help="Data sizes to test (bytes)")
    parser.add_argument("--connections", nargs='+', type=int, default=[1, 10, 50],
                        help="Number of concurrent connections")
    parser.add_argument("--operations", type=int, default=1000,
                        help="Number of operations per connection")
    parser.add_argument("--output-dir", default="benchmark_results",
                        help="Directory to save output files")
    parser.add_argument("--skip-plots", action="store_true",
                        help="Skip creating plots")

    args = parser.parse_args()

    print("🚀 Redis vs Ignix Performance Benchmark")
    print("=" * 50)

    if not check_prerequisites():
        sys.exit(1)

    servers = [
        ("localhost", 6379, "Redis"),
        ("localhost", 7379, "Ignix")
    ]
    operations = ["SET", "GET"]

    print("📋 Test Configuration:")
    print(f"   Data sizes: {args.data_sizes} bytes")
    print(f"   Concurrent connections: {args.connections}")
    print(f"   Operations per connection: {args.operations}")
    print(f"   Total number of tests: {len(servers) * len(operations) * len(args.data_sizes) * len(args.connections)}")
    print()

    # Run every configuration on both servers back to back
    all_results = []
    failures = 0
    for operation in operations:
        for data_size in args.data_sizes:
            for connections in args.connections:
                for host, port, server_name in servers:
                    try:
                        all_results.append(benchmark_server(
                            host, port, server_name, operation, data_size,
                            connections, args.operations))
                    except KeyboardInterrupt:
                        print("\n⚠️  Benchmark stopped by user!")
                        sys.exit(1)
                    except Exception as e:
                        failures += 1
                        print(f"❌ Error: {e}")

    # Save first, so a failure while reporting does not lose the results
    os.makedirs(args.output_dir, exist_ok=True)
    save_results_json(all_results, f"{args.output_dir}/benchmark_results.json")

    print_results_table(all_results)

    if not args.skip_plots:
        create_visualizations(all_results, args.output_dir)

    print(f"\n🎉 Benchmark completed! Results: {args.output_dir}/")

    if failures or any(r.error_count for r in all_results):
        sys.exit(1)


if __name__ == "__main__":
    main()
