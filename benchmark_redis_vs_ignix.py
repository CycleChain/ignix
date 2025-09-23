#!/usr/bin/env python3


import socket
import time
import threading
import statistics
import json
import os
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, asdict
from typing import List, Dict, Any, Tuple
import argparse
import subprocess


try:
    import matplotlib.pyplot as plt
    import seaborn as sns
    HAS_PLOTTING = True
except ImportError:
    HAS_PLOTTING = False
    print("⚠️  matplotlib and seaborn not found. Graphs cannot be created.")
    print("   Installation: pip install matplotlib seaborn")


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


class RedisProtocolClient:
    """Simple client for Redis RESP protocol"""
    
    def __init__(self, host: str, port: int, timeout: float = 5.0):
        self.host = host
        self.port = port
        self.timeout = timeout
        self.sock = None
        
    def connect(self) -> bool:
        """Connect to server"""
        try:
            self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self.sock.settimeout(self.timeout)
            self.sock.connect((self.host, self.port))
            return True
        except Exception as e:
            print(f"❌ {self.host}:{self.port} connection error: {e}")
            return False
    
    def disconnect(self):
        """Close connection"""
        if self.sock:
            try:
                self.sock.close()
            except:
                pass
            self.sock = None
    
    def _send_command(self, command: str, *args) -> bytes:
        """Send command in RESP format"""
        if not self.sock:
            raise Exception("No connection")
        
        # RESP array format: *<count>\r\n$<len>\r\n<data>\r\n...
        parts = [command] + list(args)
        resp_cmd = f"*{len(parts)}\r\n"
        
        for part in parts:
            part_bytes = str(part).encode('utf-8')
            resp_cmd += f"${len(part_bytes)}\r\n{part_bytes.decode('utf-8')}\r\n"
        
        self.sock.sendall(resp_cmd.encode('utf-8'))
        
        # Read response
        return self._read_response()
    
    def _read_response(self) -> bytes:
        """Read RESP response"""
        if not self.sock:
            raise Exception("No connection")
        
        # Read first character (response type)
        first_char = self.sock.recv(1)
        if not first_char:
            raise Exception("Connection closed")
        
        if first_char == b'+':  # Simple string
            line = self._read_line()
            return line
        elif first_char == b'-':  # Error
            line = self._read_line()
            raise Exception(f"Server error: {line.decode('utf-8')}")
        elif first_char == b':':  # Integer
            line = self._read_line()
            return line
        elif first_char == b'$':  # Bulk string
            length_line = self._read_line()
            length = int(length_line.decode('utf-8'))
            if length == -1:  # Null
                return b''
            data = self.sock.recv(length + 2)  # +2 for \r\n
            return data[:-2]  # Remove \r\n
        else:
            raise Exception(f"Unknown response type: {first_char}")
    
    def _read_line(self) -> bytes:
        """Read line ending with \\r\\n"""
        line = b''
        while True:
            char = self.sock.recv(1)
            if not char:
                raise Exception("Connection closed")
            if char == b'\r':
                next_char = self.sock.recv(1)
                if next_char == b'\n':
                    break
                else:
                    line += char + next_char
            else:
                line += char
        return line
    
    def set(self, key: str, value: str) -> bool:
        """SET command"""
        try:
            response = self._send_command("SET", key, value)
            return response == b'OK'
        except Exception:
            return False
    
    def get(self, key: str) -> str:
        """GET command"""
        try:
            response = self._send_command("GET", key)
            return response.decode('utf-8') if response else None
        except Exception:
            return None
    
    def ping(self) -> bool:
        """PING command - test connection"""
        try:
            response = self._send_command("PING")
            return response == b'PONG'
        except Exception:
            return False


def check_server_availability(host: str, port: int) -> bool:
    """Check if server is accessible"""
    client = RedisProtocolClient(host, port, timeout=2.0)
    if client.connect():
        result = client.ping()
        client.disconnect()
        return result
    return False


def generate_test_data(size: int) -> str:
    """Generate test data"""
    if size <= 10:
        return "x" * size
    else:
        # Use pattern for more realistic data
        pattern = "abcdefghijklmnopqrstuvwxyz0123456789"
        repeats = size // len(pattern)
        remainder = size % len(pattern)
        return pattern * repeats + pattern[:remainder]


def run_operation_batch(host: str, port: int, operation: str, 
                       keys: List[str], values: List[str], 
                       batch_size: int) -> Tuple[List[float], int]:
    """Run a batch of operations"""
    client = RedisProtocolClient(host, port)
    latencies = []
    errors = 0
    
    if not client.connect():
        return [], batch_size  # All operations failed
    
    try:
        for i in range(batch_size):
            key_idx = i % len(keys)
            start_time = time.time()
            
            if operation == "SET":
                success = client.set(keys[key_idx], values[key_idx])
            else:  # GET
                result = client.get(keys[key_idx])
                success = result is not None
            
            end_time = time.time()
            latency_ms = (end_time - start_time) * 1000
            
            if success:
                latencies.append(latency_ms)
            else:
                errors += 1
    
    finally:
        client.disconnect()
    
    return latencies, errors


def benchmark_server(host: str, port: int, server_name: str,
                    operation: str, data_size: int, 
                    concurrent_connections: int, operations_per_connection: int) -> BenchmarkResult:
    """Run benchmark for a single server"""
    
    print(f"🔄 {server_name} - {operation} benchmark starting...")
    print(f"   Data size: {data_size} bytes")
    print(f"   Concurrent connections: {concurrent_connections}")
    print(f"   Total operations: {concurrent_connections * operations_per_connection}")
    
    # Prepare test data
    test_keys = [f"benchmark_key_{i}" for i in range(1000)]
    test_values = [generate_test_data(data_size) for _ in range(1000)]
    
    # If doing GET test, first SET the data
    if operation == "GET":
        print(f"   📝 Preparing data for GET test...")
        setup_client = RedisProtocolClient(host, port)
        if setup_client.connect():
            for key, value in zip(test_keys, test_values):
                setup_client.set(key, value)
            setup_client.disconnect()
    
    # Run benchmark
    start_time = time.time()
    all_latencies = []
    total_errors = 0
    
    with ThreadPoolExecutor(max_workers=concurrent_connections) as executor:
        futures = []
        
        for _ in range(concurrent_connections):
            future = executor.submit(
                run_operation_batch,
                host, port, operation,
                test_keys, test_values,
                operations_per_connection
            )
            futures.append(future)
        
        # Collect results
        for future in as_completed(futures):
            latencies, errors = future.result()
            all_latencies.extend(latencies)
            total_errors += errors
    
    end_time = time.time()
    total_time = end_time - start_time
    total_operations = concurrent_connections * operations_per_connection
    successful_operations = len(all_latencies)
    
    # Calculate statistics
    if all_latencies:
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
    print(f"   Success rate: {success_rate*100:.1f}%")
    print()
    
    return result


def print_results_table(results: List[BenchmarkResult]):
    """Print results in table format"""
    print("\n" + "="*120)
    print("🏆 BENCHMARK RESULTS")
    print("="*120)
    
    # Header
    print(f"{'Server':<10} {'Operation':<9} {'Data Size':<10} {'Connections':<11} "
          f"{'Ops/sec':<10} {'Avg Lat(ms)':<12} {'P95(ms)':<9} {'P99(ms)':<9} {'Success%':<9}")
    print("-"*120)
    
    # Results
    for result in results:
        print(f"{result.server_name:<10} {result.operation:<9} {result.data_size:<10} "
              f"{result.concurrent_connections:<11} {result.operations_per_second:<10.1f} "
              f"{result.avg_latency_ms:<12.2f} {result.p95_latency_ms:<9.2f} "
              f"{result.p99_latency_ms:<9.2f} {result.success_rate*100:<9.1f}")
    
    print("-"*120)
    
    # Comparison
    print("\n🔍 COMPARISON SUMMARY:")
    print("-"*50)
    
    operations = list(set(r.operation for r in results))
    data_sizes = list(set(r.data_size for r in results))
    
    for operation in operations:
        for data_size in data_sizes:
            op_results = [r for r in results if r.operation == operation and r.data_size == data_size]
            if len(op_results) >= 2:
                redis_result = next((r for r in op_results if r.server_name == "Redis"), None)
                ignix_result = next((r for r in op_results if r.server_name == "Ignix"), None)
                
                if redis_result and ignix_result:
                    ops_ratio = ignix_result.operations_per_second / redis_result.operations_per_second
                    lat_ratio = redis_result.avg_latency_ms / ignix_result.avg_latency_ms
                    
                    print(f"\n{operation} ({data_size} bytes):")
                    print(f"  Throughput: Ignix {ops_ratio:.2f}x Redis")
                    print(f"  Latency: Ignix {lat_ratio:.2f}x better" if lat_ratio > 1 else f"  Latency: Redis {1/lat_ratio:.2f}x better")


def create_visualizations(results: List[BenchmarkResult], output_dir: str = "benchmark_results"):
    """Create visualization charts"""
    if not HAS_PLOTTING:
        print("⚠️  Matplotlib not found, cannot create charts.")
        return
    
    os.makedirs(output_dir, exist_ok=True)
    
    # Style settings
    plt.style.use('seaborn-v0_8')
    sns.set_palette("husl")
    
    # 1. Throughput comparison
    fig, axes = plt.subplots(2, 2, figsize=(15, 12))
    fig.suptitle('Redis vs Ignix Performance Comparison', fontsize=16, fontweight='bold')
    
    # Operations/second comparison
    operations = ['SET', 'GET']
    servers = ['Redis', 'Ignix']
    
    for i, operation in enumerate(operations):
        op_results = [r for r in results if r.operation == operation]
        data_sizes = sorted(list(set(r.data_size for r in op_results)))
        
        redis_ops = []
        ignix_ops = []
        
        for size in data_sizes:
            redis_result = next((r for r in op_results if r.server_name == "Redis" and r.data_size == size), None)
            ignix_result = next((r for r in op_results if r.server_name == "Ignix" and r.data_size == size), None)
            
            redis_ops.append(redis_result.operations_per_second if redis_result else 0)
            ignix_ops.append(ignix_result.operations_per_second if ignix_result else 0)
        
        x = range(len(data_sizes))
        width = 0.35
        
        axes[0, i].bar([xi - width/2 for xi in x], redis_ops, width, label='Redis', alpha=0.8)
        axes[0, i].bar([xi + width/2 for xi in x], ignix_ops, width, label='Ignix', alpha=0.8)
        
        axes[0, i].set_title(f'{operation} Operations per Second')
        axes[0, i].set_xlabel('Data Size (bytes)')
        axes[0, i].set_ylabel('Operations/second')
        axes[0, i].set_xticks(x)
        axes[0, i].set_xticklabels([str(s) for s in data_sizes])
        axes[0, i].legend()
        axes[0, i].grid(True, alpha=0.3)
    
    # Latency comparison
    for i, operation in enumerate(operations):
        op_results = [r for r in results if r.operation == operation]
        data_sizes = sorted(list(set(r.data_size for r in op_results)))
        
        redis_lat = []
        ignix_lat = []
        
        for size in data_sizes:
            redis_result = next((r for r in op_results if r.server_name == "Redis" and r.data_size == size), None)
            ignix_result = next((r for r in op_results if r.server_name == "Ignix" and r.data_size == size), None)
            
            redis_lat.append(redis_result.avg_latency_ms if redis_result else 0)
            ignix_lat.append(ignix_result.avg_latency_ms if ignix_result else 0)
        
        x = range(len(data_sizes))
        width = 0.35
        
        axes[1, i].bar([xi - width/2 for xi in x], redis_lat, width, label='Redis', alpha=0.8)
        axes[1, i].bar([xi + width/2 for xi in x], ignix_lat, width, label='Ignix', alpha=0.8)
        
        axes[1, i].set_title(f'{operation} Average Latency')
        axes[1, i].set_xlabel('Data Size (bytes)')
        axes[1, i].set_ylabel('Latency (ms)')
        axes[1, i].set_xticks(x)
        axes[1, i].set_xticklabels([str(s) for s in data_sizes])
        axes[1, i].legend()
        axes[1, i].grid(True, alpha=0.3)
    
    plt.tight_layout()
    plt.savefig(f"{output_dir}/redis_vs_ignix_comparison.png", dpi=300, bbox_inches='tight')
    plt.close()
    
    # 2. Performance ratio grafiği
    fig, ax = plt.subplots(1, 1, figsize=(12, 8))
    
    ratios_data = []
    labels = []
    
    for operation in operations:
        op_results = [r for r in results if r.operation == operation]
        data_sizes = sorted(list(set(r.data_size for r in op_results)))
        
        for size in data_sizes:
            redis_result = next((r for r in op_results if r.server_name == "Redis" and r.data_size == size), None)
            ignix_result = next((r for r in op_results if r.server_name == "Ignix" and r.data_size == size), None)
            
            if redis_result and ignix_result:
                ratio = ignix_result.operations_per_second / redis_result.operations_per_second
                ratios_data.append(ratio)
                labels.append(f"{operation}\n{size}B")
    
    colors = ['green' if r > 1 else 'red' for r in ratios_data]
    bars = ax.bar(labels, ratios_data, color=colors, alpha=0.7)
    
    ax.axhline(y=1, color='black', linestyle='--', alpha=0.5, label='Equal Performance')
    ax.set_title('Ignix vs Redis Performance Ratio\n(>1 means Ignix is faster)', fontweight='bold')
    ax.set_ylabel('Performance Ratio (Ignix/Redis)')
    ax.set_xlabel('Test Configuration')
    ax.grid(True, alpha=0.3)
    
    # Değerleri bar'ların üstüne yaz
    for bar, ratio in zip(bars, ratios_data):
        height = bar.get_height()
        ax.text(bar.get_x() + bar.get_width()/2., height + 0.01,
                f'{ratio:.2f}x', ha='center', va='bottom', fontweight='bold')
    
    plt.tight_layout()
    plt.savefig(f"{output_dir}/performance_ratio.png", dpi=300, bbox_inches='tight')
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
    
    # Redis check
    redis_available = check_server_availability("localhost", 6379)
    print(f"{'✅' if redis_available else '❌'} Redis (localhost:6379): {'Accessible' if redis_available else 'Not accessible'}")
    
    # Ignix check  
    ignix_available = check_server_availability("localhost", 7379)
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
    
    # Prerequisites check
    if not check_prerequisites():
        sys.exit(1)
    
    # Test configuration
    servers = [
        ("localhost", 6379, "Redis"),
        ("localhost", 7379, "Ignix")
    ]
    
    operations = ["SET", "GET"]
    
    print(f"📋 Test Configuration:")
    print(f"   Data sizes: {args.data_sizes} bytes")
    print(f"   Concurrent connections: {args.connections}")
    print(f"   Operations per connection: {args.operations}")
    print(f"   Total number of tests: {len(servers) * len(operations) * len(args.data_sizes) * len(args.connections)}")
    print()
    
    # Run benchmarks
    all_results = []
    
    for host, port, server_name in servers:
        for operation in operations:
            for data_size in args.data_sizes:
                for connections in args.connections:
                    try:
                        result = benchmark_server(
                            host, port, server_name,
                            operation, data_size,
                            connections, args.operations
                        )
                        all_results.append(result)
                    except KeyboardInterrupt:
                        print("\n⚠️  Benchmark stopped by user!")
                        sys.exit(1)
                    except Exception as e:
                        print(f"❌ Error: {e}")
                        continue
    
    # Show results
    print_results_table(all_results)
    
    # Save results
    os.makedirs(args.output_dir, exist_ok=True)
    save_results_json(all_results, f"{args.output_dir}/benchmark_results.json")
    
    # Create charts
    if not args.skip_plots:
        create_visualizations(all_results, args.output_dir)
    
    print(f"\n🎉 Benchmark completed! Results: {args.output_dir}/")


if __name__ == "__main__":
    main()
