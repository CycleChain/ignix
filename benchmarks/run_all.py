#!/usr/bin/env python3

import os
import sys
import subprocess
import time
import webbrowser
from datetime import datetime

def print_header(msg):
    print("\n" + "="*60)
    print(f"🚀 {msg}")
    print("="*60)

def run_command(cmd, cwd=None):
    print(f"   Running: {' '.join(cmd)}")
    try:
        subprocess.run(cmd, cwd=cwd, check=True)
        return True
    except subprocess.CalledProcessError as e:
        print(f"   ❌ Error: {e}")
        return False

def check_server(host, port, name):
    import socket
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.settimeout(1)
        s.connect((host, port))
        s.close()
        print(f"   ✅ {name} is running on {host}:{port}")
        return True
    except:
        print(f"   ❌ {name} is NOT running on {host}:{port}")
        return False

def main():
    base_dir = os.path.dirname(os.path.abspath(__file__))
    scripts_dir = os.path.join(base_dir, "scripts")
    results_dir = os.path.join(base_dir, "results")
    
    print_header("Ignix Unified Benchmark Runner")
    
    # 1. Check Prerequisites
    print("\n🔍 Checking prerequisites...")
    redis_ok = check_server("localhost", 6379, "Redis")
    ignix_ok = check_server("localhost", 7379, "Ignix")
    
    if not (redis_ok and ignix_ok):
        print("\n⚠️  Please start both Redis and Ignix servers before running benchmarks.")
        print("   Redis: redis-server")
        print("   Ignix: cargo run --release")
        sys.exit(1)

    # 2. Run Basic Benchmark
    print_header("Running Basic Benchmark")
    run_command(
        [sys.executable, "basic_benchmark.py", "--output-dir", os.path.join(results_dir, "basic"), "--skip-plots"],
        cwd=scripts_dir
    )

    # 3. Run Comprehensive Benchmark
    print_header("Running Comprehensive Benchmark")
    run_command(
        [sys.executable, "comprehensive_benchmark.py", "--out", os.path.join(results_dir, "comprehensive")],
        cwd=scripts_dir
    )

    # 4. Run Real-World Benchmark
    print_header("Running Real-World Benchmark")
    run_command(
        [sys.executable, "real_world_benchmark.py", "--out", os.path.join(results_dir, "real_world")],
        cwd=scripts_dir
    )

    # 5. Generate Report
    print_header("Generating Report")
    report_path = os.path.join(results_dir, "index.html")
    
    html_content = f"""
    <!DOCTYPE html>
    <html>
    <head>
        <title>Ignix Benchmark Report</title>
        <style>
            body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif; margin: 0; padding: 20px; background: #f5f5f7; color: #1d1d1f; }}
            .container {{ max_width: 1200px; margin: 0 auto; background: white; padding: 40px; border-radius: 18px; box-shadow: 0 4px 20px rgba(0,0,0,0.05); }}
            h1 {{ text-align: center; color: #1d1d1f; margin-bottom: 40px; }}
            h2 {{ border-bottom: 2px solid #f5f5f7; padding-bottom: 10px; margin-top: 40px; }}
            .section {{ margin-bottom: 40px; }}
            .grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(500px, 1fr)); gap: 20px; }}
            .card {{ background: #fff; border: 1px solid #e5e5e5; border-radius: 12px; padding: 20px; text-align: center; }}
            img {{ max_width: 100%; height: auto; border-radius: 8px; }}
            .timestamp {{ text-align: center; color: #86868b; margin-bottom: 40px; }}
        </style>
    </head>
    <body>
        <div class="container">
            <h1>Ignix Performance Report</h1>
            <p class="timestamp">Generated on {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}</p>
            
            <div class="section">
                <h2>1. Comprehensive Benchmark (Synthetic)</h2>
                <p>Detailed analysis of throughput and latency stability.</p>
                <div class="grid">
                    <div class="card">
                        <h3>Throughput Comparison</h3>
                        <img src="comprehensive/throughput.png" alt="Throughput">
                    </div>
                    <div class="card">
                        <h3>Latency Distribution</h3>
                        <img src="comprehensive/latency_dist.png" alt="Latency Distribution">
                    </div>
                </div>
            </div>

            <div class="section">
                <h2>2. Real-World Benchmark (Session Store)</h2>
                <p>Simulation of high-traffic session store with Zipfian key distribution.</p>
                <div class="grid">
                    <div class="card">
                        <h3>Throughput</h3>
                        <img src="real_world/real_world_throughput.png" alt="Real World Throughput">
                    </div>
                    <div class="card">
                        <h3>Latency Distribution</h3>
                        <img src="real_world/real_world_latency.png" alt="Real World Latency">
                    </div>
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

if __name__ == "__main__":
    main()
