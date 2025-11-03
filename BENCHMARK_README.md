# Redis vs Ignix Performance Benchmark

This benchmark system performs detailed performance comparison between Redis and Ignix for GET and SET commands.

## 🚀 Quick Start

### 1. Prerequisites

**Start Redis (port 6379):**
```bash
redis-server
```

**Start Ignix (port 7379):**
```bash
cargo run --release
```

### 2. Run Benchmark

**Simple test:**
```bash
python3 benchmark_redis_vs_ignix.py
```

**Advanced test (with charts):**
```bash
# Install graphics libraries
pip install matplotlib seaborn

# Run benchmark
python3 benchmark_redis_vs_ignix.py --data-sizes 64 256 1024 --connections 1 10 50
```

## 📊 Features

### Core Features
- ✅ Redis and Ignix comparison
- ✅ GET and SET operation tests
- ✅ Multiple data size support (64B - 4KB)
- ✅ Concurrent connection tests (1-50 connections)
- ✅ Detailed latency statistics (avg, p95, p99)
- ✅ Throughput measurement (ops/second)
- ✅ Success rate tracking

### Advanced Features
- 📊 Automatic visualization charts
- 💾 JSON export
- 🔍 Server accessibility check
- 📈 Performance ratio analysis
- 🎯 Detailed comparison table

## 🛠️ Usage

### Basic Usage
```bash
python3 benchmark_redis_vs_ignix.py
```

### Custom Test
```bash
python3 benchmark_redis_vs_ignix.py \
  --data-sizes 128 512 2048 \
  --connections 5 25 100 \
  --operations 2000 \
  --output-dir my_benchmark_results
```

### Parameters

| Parameter | Description | Default |
|-----------|-------------|---------|
| `--data-sizes` | Data sizes to test (bytes) | `64 256 1024 4096` |
| `--connections` | Number of concurrent connections | `1 10 50` |
| `--operations` | Number of operations per connection | `1000` |
| `--output-dir` | Directory to save output files | `benchmark_results` |
| `--skip-plots` | Skip creating plots | `False` |

## 📈 Output Examples

### Summary Tables (from benchmark_results/benchmark_results.json)

#### SET Throughput (ops/sec)

| Data | Conns | Redis | Ignix | Ratio (Ignix/Redis) |
|------|-------|-------|-------|----------------------|
| 64B  | 1     | 9,249 | 9,116 | 0.99x |
| 64B  | 10    | 17,628 | 21,545 | 1.22x |
| 64B  | 50    | 18,236 | 15,515 | 0.85x |
| 256B | 1     | 14,615 | 12,032 | 0.82x |
| 256B | 10    | 17,880 | 28,032 | 1.57x |
| 256B | 50    | 16,898 | 19,306 | 1.14x |
| 1KB  | 1     | 16,300 | 11,083 | 0.68x |
| 1KB  | 10    | 16,936 | 20,644 | 1.22x |
| 1KB  | 50    | 17,155 | 20,663 | 1.20x |
| 4KB  | 1     | 11,286 | 8,581 | 0.76x |
| 4KB  | 10    | 17,232 | 28,136 | 1.63x |
| 4KB  | 50    | 16,343 | 17,131 | 1.05x |

#### GET Throughput (ops/sec)

| Data | Conns | Redis | Ignix | Ratio (Ignix/Redis) |
|------|-------|-------|-------|----------------------|
| 64B  | 1     | 19,612 | 11,129 | 0.57x |
| 64B  | 10    | 16,780 | 28,873 | 1.72x |
| 64B  | 50    | 14,948 | 19,609 | 1.31x |
| 256B | 1     | 20,035 | 11,961 | 0.60x |
| 256B | 10    | 15,164 | 26,699 | 1.76x |
| 256B | 50    | 15,619 | 18,508 | 1.18x |
| 1KB  | 1     | 17,525 | 10,436 | 0.60x |
| 1KB  | 10    | 11,930 | 23,184 | 1.94x |
| 1KB  | 50    | 8,138 | 15,321 | 1.88x |
| 4KB  | 1     | 16,600 | 12,159 | 0.73x |
| 4KB  | 10    | 7,532 | 13,937 | 1.85x |
| 4KB  | 50    | 13,035 | 16,354 | 1.25x |

Notes:
- Values rounded from `benchmark_results/benchmark_results.json`.
- All runs in this dataset reported 0 errors (100% success).

### Console Output
```
🚀 Redis vs Ignix Performance Benchmark
==================================================

🔍 Checking prerequisites...
✅ Redis (localhost:6379): Accessible
✅ Ignix (localhost:7379): Accessible
✅ All prerequisites met!

🔄 Redis - SET benchmark starting...
   Data size: 64 bytes
   Concurrent connections: 10
   Total operations: 10000
✅ Redis - SET completed!
   Operations/second: 45231.2
   Average latency: 0.22 ms
   Success rate: 100.0%

========================================================
🏆 BENCHMARK RESULTS
========================================================
Server     Operation Data Size  Connections Ops/sec    Avg Lat(ms) P95(ms)  P99(ms)  Success%
--------------------------------------------------------
Redis      SET       64         10          45231.2    0.22        0.35     0.48     100.0
Ignix      SET       64         10          52147.8    0.19        0.31     0.42     100.0
Redis      GET       64         10          48923.1    0.20        0.33     0.45     100.0
Ignix      GET       64         10          51234.7    0.19        0.30     0.41     100.0

🔍 COMPARISON SUMMARY:
--------------------------------------------------

SET (64 bytes):
  Throughput: Ignix 1.15x Redis
  Latency: Ignix 1.16x better

GET (64 bytes):
  Throughput: Ignix 1.05x Redis
  Latency: Ignix 1.05x better
```

### Generated Files

**benchmark_results/** directory:
- `benchmark_results.json` - Raw data
- `redis_vs_ignix_comparison.png` - Main comparison charts
- `performance_ratio.png` - Performance ratio chart

## 🔧 Troubleshooting

### Server Connection Issues

**Redis connection error:**
```bash
# Check if Redis is running
redis-cli ping

# Start Redis
redis-server
```

**Ignix connection error:**
```bash
# Check if Ignix is running
lsof -i :7379

# Start Ignix
cargo run --release
```

### Python Dependency Issues

**Matplotlib installation error:**
```bash
# macOS
brew install python-tk
pip install matplotlib seaborn

# Ubuntu/Debian
sudo apt-get install python3-tk
pip install matplotlib seaborn
```

**Permission denied error:**
```bash
# Use virtual environment
python3 -m venv benchmark_env
source benchmark_env/bin/activate
pip install matplotlib seaborn
```

### Performance Issues

**Low throughput:**
- Check system load: `top`
- Check network latency: `ping localhost`
- Try fewer concurrent connections: `--connections 1 5`

**High error rate:**
- Check server logs
- Increase timeout values (in code `timeout=5.0`)
- Try fewer operations: `--operations 500`

## 📊 Chart Examples

### Throughput Comparison
- ops/second for SET and GET operations
- Performance across different data sizes
- Redis vs Ignix side-by-side comparison

### Latency Analysis
- Average, P95, P99 latency values
- Latency changes by data size
- Server comparative latency charts

### Performance Ratio
- Ignix/Redis performance ratio
- Values >1 indicate Ignix is faster
- Detailed breakdown by test configuration

## 🎯 Test Scenarios

### Quick Test (30 seconds)
```bash
python3 benchmark_redis_vs_ignix.py --data-sizes 64 --connections 1 --operations 1000
```

### Medium Test (5 minutes)
```bash
python3 benchmark_redis_vs_ignix.py --data-sizes 64 256 1024 --connections 1 10 --operations 1000
```

### Comprehensive Test (15 minutes)
```bash
python3 benchmark_redis_vs_ignix.py --data-sizes 64 256 1024 4096 --connections 1 10 25 50 --operations 2000
```

### Stress Test (30 minutes)
```bash
python3 benchmark_redis_vs_ignix.py --data-sizes 64 256 1024 4096 8192 --connections 1 10 25 50 100 --operations 5000
```

## 📝 Notes

- Do not run other intensive processes during benchmarking
- Test results may vary depending on system performance
- Run multiple times for reliable results
- Take backups before testing in production environment

## 🤝 Contributing

To improve the benchmark:
1. Add new test scenarios
2. Implement additional metrics
3. Improve chart visualizations
4. Share bug reports and suggestions