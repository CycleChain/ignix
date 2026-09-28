#!/usr/bin/env python3
"""Run the basic, comprehensive and real-world benchmarks against Redis
(localhost:6379) and Ignix (localhost:7379) and write an HTML index.

Results go to benchmarks/results/{basic,comprehensive,real_world}/. The
script exits with a non-zero status if a server is missing or any
benchmark failed or saw errors.
"""

import os
import subprocess
import sys
from datetime import datetime

SCRIPTS_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "scripts")
sys.path.insert(0, SCRIPTS_DIR)

from resp_client import ping  # noqa: E402


def print_header(msg):
    print("\n" + "="*60)
    print(f"🚀 {msg}")
    print("="*60)


def run_command(cmd, cwd=None) -> bool:
    print(f"   Running: {' '.join(cmd)}")
    result = subprocess.run(cmd, cwd=cwd)
    if result.returncode != 0:
        print(f"   ❌ Exited with status {result.returncode}")
        return False
    return True


def check_server(host, port, name) -> bool:
    if ping(host, port):
        print(f"   ✅ {name} is running on {host}:{port}")
        return True
    print(f"   ❌ {name} does not answer PING on {host}:{port}")
    return False


def image_card(results_dir: str, path: str, title: str) -> str:
    if not os.path.exists(os.path.join(results_dir, path)):
        return f'<div class="card"><h3>{title}</h3><p>Not generated (plotting libraries missing or the run failed).</p></div>'
    return f'<div class="card"><h3>{title}</h3><img src="{path}" alt="{title}"></div>'


def main():
    base_dir = os.path.dirname(os.path.abspath(__file__))
    results_dir = os.path.join(base_dir, "results")

    print_header("Ignix Unified Benchmark Runner")

    # Check for plotting dependencies
    try:
        import matplotlib  # noqa: F401
        import pandas  # noqa: F401
        import seaborn  # noqa: F401
        import numpy  # noqa: F401
    except ImportError as e:
        print(f"\n⚠️  Warning: Missing dependency for plotting: {e.name}")
        print("   Graphs will NOT be generated. To fix: pip install matplotlib pandas seaborn numpy")

    # 1. Check Prerequisites
    print("\n🔍 Checking prerequisites...")
    redis_ok = check_server("localhost", 6379, "Redis")
    ignix_ok = check_server("localhost", 7379, "Ignix")

    if not (redis_ok and ignix_ok):
        print("\n⚠️  Please start both Redis and Ignix servers before running benchmarks.")
        print("   Redis: redis-server")
        print("   Ignix: cargo run --release")
        sys.exit(1)

    ok = True

    # 2. Run Basic Benchmark
    print_header("Running Basic Benchmark")
    ok &= run_command(
        [sys.executable, "basic_benchmark.py", "--output-dir", os.path.join(results_dir, "basic")],
        cwd=SCRIPTS_DIR
    )

    # 3. Run Comprehensive Benchmark
    print_header("Running Comprehensive Benchmark")
    comprehensive_dir = os.path.join(results_dir, "comprehensive")
    ok &= run_command(
        [sys.executable, "comprehensive_benchmark.py", "--out", comprehensive_dir,
         "--json-out", os.path.join(comprehensive_dir, "benchmark_results.json")],
        cwd=SCRIPTS_DIR
    )

    # 4. Run Real-World Benchmark
    print_header("Running Real-World Benchmark")
    real_world_dir = os.path.join(results_dir, "real_world")
    ok &= run_command(
        [sys.executable, "real_world_benchmark.py", "--out", real_world_dir,
         "--json-out", os.path.join(real_world_dir, "real_world_results.json")],
        cwd=SCRIPTS_DIR
    )

    # 5. Generate Report
    print_header("Generating Report")
    os.makedirs(results_dir, exist_ok=True)
    report_path = os.path.join(results_dir, "index.html")

    cards_comprehensive = "\n".join([
        image_card(results_dir, "comprehensive/throughput.png", "Throughput Comparison"),
        image_card(results_dir, "comprehensive/latency_dist.png", "Latency Distribution"),
    ])
    cards_real_world = "\n".join([
        image_card(results_dir, "real_world/real_world_throughput.png", "Throughput"),
        image_card(results_dir, "real_world/real_world_latency.png", "Latency Distribution"),
    ])

    html_content = f"""<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Ignix Benchmark Report</title>
    <style>
        body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif; margin: 0; padding: 20px; background: #f5f5f7; color: #1d1d1f; }}
        .container {{ max-width: 1200px; margin: 0 auto; background: white; padding: 40px; border-radius: 18px; box-shadow: 0 4px 20px rgba(0,0,0,0.05); }}
        h1 {{ text-align: center; color: #1d1d1f; margin-bottom: 40px; }}
        h2 {{ border-bottom: 2px solid #f5f5f7; padding-bottom: 10px; margin-top: 40px; }}
        .section {{ margin-bottom: 40px; }}
        .grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(500px, 1fr)); gap: 20px; }}
        .card {{ background: #fff; border: 1px solid #e5e5e5; border-radius: 12px; padding: 20px; text-align: center; }}
        img {{ max-width: 100%; height: auto; border-radius: 8px; }}
        .timestamp {{ text-align: center; color: #86868b; margin-bottom: 40px; }}
    </style>
</head>
<body>
    <div class="container">
        <h1>Ignix Performance Report</h1>
        <p class="timestamp">Generated on {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}</p>

        <div class="section">
            <h2>1. Comprehensive Benchmark (Synthetic)</h2>
            <p>Throughput and latency per value size. Raw numbers: comprehensive/benchmark_results.json</p>
            <div class="grid">
{cards_comprehensive}
            </div>
        </div>

        <div class="section">
            <h2>2. Real-World Benchmark (Session Store)</h2>
            <p>Mixed 80/20 GET/SET workload with a Zipfian key distribution. Raw numbers: real_world/real_world_results.json</p>
            <div class="grid">
{cards_real_world}
            </div>
        </div>
    </div>
</body>
</html>
"""

    with open(report_path, "w") as f:
        f.write(html_content)

    print(f"✅ Report generated: {report_path}")
    print(f"   Open file://{report_path} in your browser to view results.")

    if not ok:
        print("\n❌ At least one benchmark failed or reported errors; see the output above.")
        sys.exit(1)


if __name__ == "__main__":
    main()
