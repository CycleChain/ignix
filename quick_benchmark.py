#!/usr/bin/env python3
"""
Redis vs Ignix Quick Benchmark
===============================

Use for quick and simple benchmark testing.
Only performs basic comparison, does not create graphs.

Usage:
    python3 quick_benchmark.py
"""

import socket
import time
import statistics
from typing import List, Tuple


class SimpleClient:
    """Simple Redis protocol client"""
    
    def __init__(self, host: str, port: int):
        self.host = host
        self.port = port
        self.sock = None
    
    def connect(self) -> bool:
        try:
            self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self.sock.settimeout(5.0)
            self.sock.connect((self.host, self.port))
            return True
        except:
            return False
    
    def disconnect(self):
        if self.sock:
            self.sock.close()
            self.sock = None
    
    def _send_command(self, *parts) -> str:
        if not self.sock:
            return ""
        
        # RESP format
        cmd = f"*{len(parts)}\r\n"
        for part in parts:
            part_bytes = str(part).encode('utf-8')
            cmd += f"${len(part_bytes)}\r\n{part_bytes.decode('utf-8')}\r\n"
        
        self.sock.sendall(cmd.encode('utf-8'))
        
        # Read response
        response = b""
        while True:
            chunk = self.sock.recv(1024)
            if not chunk:
                break
            response += chunk
            if response.endswith(b'\r\n'):
                break
        
        return response.decode('utf-8', errors='ignore').strip()
    
    def set(self, key: str, value: str) -> bool:
        try:
            response = self._send_command("SET", key, value)
            return "OK" in response
        except:
            return False
    
    def get(self, key: str) -> bool:
        try:
            response = self._send_command("GET", key)
            return response and response != "$-1"  # Not null
        except:
            return False


def benchmark_server(host: str, port: int, name: str, operations: int = 1000) -> Tuple[float, float, float]:
    """
    Server benchmark
    Returns: (set_ops_per_sec, get_ops_per_sec, success_rate)
    """
    client = SimpleClient(host, port)
    
    if not client.connect():
        print(f"❌ {name} ({host}:{port}) connection failed!")
        return 0, 0, 0
    
    print(f"🔄 {name} benchmark starting... ({operations} operations)")
    
    # SET test
    set_times = []
    set_errors = 0
    
    for i in range(operations):
        start = time.time()
        success = client.set(f"bench_key_{i}", f"test_value_{i}_{'x' * 50}")
        end = time.time()
        
        if success:
            set_times.append(end - start)
        else:
            set_errors += 1
    
    # GET test  
    get_times = []
    get_errors = 0
    
    for i in range(operations):
        start = time.time()
        success = client.get(f"bench_key_{i}")
        end = time.time()
        
        if success:
            get_times.append(end - start)
        else:
            get_errors += 1
    
    client.disconnect()
    
    # Calculate statistics
    set_ops_per_sec = len(set_times) / sum(set_times) if set_times else 0
    get_ops_per_sec = len(get_times) / sum(get_times) if get_times else 0
    success_rate = (len(set_times) + len(get_times)) / (operations * 2)
    
    avg_set_latency = statistics.mean(set_times) * 1000 if set_times else 0
    avg_get_latency = statistics.mean(get_times) * 1000 if get_times else 0
    
    print(f"✅ {name} completed!")
    print(f"   SET: {set_ops_per_sec:.0f} ops/sec, {avg_set_latency:.2f} ms avg")
    print(f"   GET: {get_ops_per_sec:.0f} ops/sec, {avg_get_latency:.2f} ms avg")
    print(f"   Success: {success_rate*100:.1f}%")
    print()
    
    return set_ops_per_sec, get_ops_per_sec, success_rate


def main():
    print("🚀 Redis vs Ignix Quick Benchmark")
    print("=" * 40)
    
    # Server accessibility check
    servers = [
        ("localhost", 6379, "Redis"),
        ("localhost", 7379, "Ignix")
    ]
    
    available_servers = []
    for host, port, name in servers:
        client = SimpleClient(host, port)
        if client.connect():
            client.disconnect()
            print(f"✅ {name} ({host}:{port}) accessible")
            available_servers.append((host, port, name))
        else:
            print(f"❌ {name} ({host}:{port}) not accessible")
    
    if len(available_servers) < 2:
        print("\n⚠️  Both servers must be running!")
        print("   Redis: redis-server")
        print("   Ignix: cargo run --release")
        return
    
    print()
    
    # Run benchmarks
    results = []
    for host, port, name in available_servers:
        set_ops, get_ops, success_rate = benchmark_server(host, port, name)
        results.append((name, set_ops, get_ops, success_rate))
    
    # Compare results
    if len(results) >= 2:
        print("🏆 COMPARISON")
        print("-" * 40)
        
        redis_result = next((r for r in results if r[0] == "Redis"), None)
        ignix_result = next((r for r in results if r[0] == "Ignix"), None)
        
        if redis_result and ignix_result:
            _, redis_set, redis_get, redis_success = redis_result
            _, ignix_set, ignix_get, ignix_success = ignix_result
            
            if redis_set > 0 and redis_get > 0:
                set_ratio = ignix_set / redis_set
                get_ratio = ignix_get / redis_get
                
                print(f"SET Performance: Ignix {set_ratio:.2f}x Redis")
                print(f"GET Performance: Ignix {get_ratio:.2f}x Redis")
                
                if set_ratio > 1:
                    print(f"🎉 Ignix is {(set_ratio-1)*100:.1f}% faster in SET operations!")
                else:
                    print(f"📊 Redis is {(1/set_ratio-1)*100:.1f}% faster in SET operations!")
                
                if get_ratio > 1:
                    print(f"🎉 Ignix is {(get_ratio-1)*100:.1f}% faster in GET operations!")
                else:
                    print(f"📊 Redis is {(1/get_ratio-1)*100:.1f}% faster in GET operations!")
    
    print("\n💡 For detailed benchmark:")
    print("   python3 benchmark_redis_vs_ignix.py")


if __name__ == "__main__":
    main()
