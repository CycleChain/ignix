#!/usr/bin/env python3
"""
Redis vs Ignix Quick Benchmark
===============================

A quick single-connection comparison: 1000 SETs and then 1000 GETs of a
~64-byte value on each server. Every reply is checked (SET must answer OK,
GET must return the value that was written) and errors are reported. No
graphs are created; for detailed runs use run_all.py or the scripts in
scripts/.

Usage:
    python3 benchmarks/quick_benchmark.py
"""

import os
import statistics
import sys
import time
from typing import List, Optional, Tuple

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "scripts"))

from resp_client import ProtocolError, RespClient, RespError, encode_command, ping  # noqa: E402

OPERATIONS = 1000


def timed_requests(client: RespClient, requests: List[bytes],
                   expected: List[bytes]) -> Tuple[List[float], List[str]]:
    """Send requests one by one; return latencies of correct replies and errors."""
    latencies, errors = [], []
    for request, want in zip(requests, expected):
        t0 = time.perf_counter()
        try:
            reply = client.request(request)
        except RespError as e:
            reply = e
        t1 = time.perf_counter()
        if reply == want:
            latencies.append(t1 - t0)
        else:
            errors.append(str(reply)[:80])
    return latencies, errors


def report(op: str, latencies: List[float], errors: List[str]) -> float:
    """Print one line for an operation and return its ops/sec."""
    ops = len(latencies) / sum(latencies) if latencies else 0.0
    avg = statistics.mean(latencies) * 1000 if latencies else 0.0
    print(f"   {op}: {ops:.0f} ops/sec, {avg:.3f} ms avg, {len(errors)} errors")
    if errors:
        print(f"      first error: {errors[0]}")
    return ops


def benchmark_server(host: str, port: int, name: str) -> Optional[Tuple[float, float, int]]:
    """Return (set ops/sec, get ops/sec, errors), or None if the run failed."""
    keys = [f"bench_key_{i}" for i in range(OPERATIONS)]
    values = [f"test_value_{i}_{'x' * 50}".encode() for i in range(OPERATIONS)]
    set_requests = [encode_command("SET", k, v) for k, v in zip(keys, values)]
    get_requests = [encode_command("GET", k) for k in keys]

    print(f"🔄 {name} benchmark starting... ({OPERATIONS} operations)")
    try:
        with RespClient(host, port) as client:
            set_lat, set_err = timed_requests(client, set_requests, [b"OK"] * OPERATIONS)
            get_lat, get_err = timed_requests(client, get_requests, values)
    except (OSError, ProtocolError, ValueError) as e:
        print(f"❌ {name}: connection failed during the benchmark: {e}")
        return None

    set_ops = report("SET", set_lat, set_err)
    get_ops = report("GET", get_lat, get_err)
    print()
    return set_ops, get_ops, len(set_err) + len(get_err)


def main():
    print("🚀 Redis vs Ignix Quick Benchmark")
    print("=" * 40)

    servers = [
        ("localhost", 6379, "Redis"),
        ("localhost", 7379, "Ignix")
    ]

    for host, port, name in servers:
        if ping(host, port):
            print(f"✅ {name} ({host}:{port}) accessible")
        else:
            print(f"❌ {name} ({host}:{port}) does not answer PING")
            print("\n⚠️  Both servers must be running!")
            print("   Redis: redis-server")
            print("   Ignix: cargo run --release")
            sys.exit(1)
    print()

    results = {name: benchmark_server(host, port, name) for host, port, name in servers}
    redis_result, ignix_result = results["Redis"], results["Ignix"]

    if redis_result and ignix_result:
        print("🏆 COMPARISON")
        print("-" * 40)
        for label, r_ops, i_ops in [("SET", redis_result[0], ignix_result[0]),
                                    ("GET", redis_result[1], ignix_result[1])]:
            if r_ops > 0 and i_ops > 0:
                ratio = i_ops / r_ops
                print(f"{label} Performance: Ignix {ratio:.2f}x Redis")
            else:
                print(f"{label} Performance: n/a (no successful operations)")

    print("\n💡 For detailed benchmarks:")
    print("   python3 benchmarks/run_all.py")

    if not (redis_result and ignix_result) or redis_result[2] or ignix_result[2]:
        sys.exit(1)


if __name__ == "__main__":
    main()
