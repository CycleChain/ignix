#!/usr/bin/env bash
# Build Ignix in release mode, start it (and Redis, if port 6379 is free and
# redis-server is installed) in temporary directories, run run_all.py and
# stop only the processes this script started.
#
# Redis is started with AOF (appendfsync everysec) so both servers persist
# writes the same way. Run from anywhere: bash benchmarks/run_benchmarks.sh
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"

port_in_use() {
  lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
}

wait_for_ping() {
  python3 - "$1" <<'EOF'
import sys, time
sys.path.insert(0, "benchmarks/scripts")
from resp_client import ping
port = int(sys.argv[1])
for _ in range(100):
    if ping("127.0.0.1", port):
        sys.exit(0)
    time.sleep(0.1)
sys.exit(1)
EOF
}

if port_in_use 7379; then
  echo "Port 7379 is already in use; stop that server first (lsof -nP -iTCP:7379 -sTCP:LISTEN)." >&2
  exit 1
fi

echo "Compiling..."
cargo build --release

workdir="$(mktemp -d)"
pids=()
cleanup() {
  for pid in "${pids[@]}"; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  rm -rf "$workdir"
}
trap cleanup EXIT

echo "Starting Ignix..."
mkdir -p "$workdir/ignix"
(cd "$workdir/ignix" && exec "$repo/target/release/ignix") >"$workdir/ignix.log" 2>&1 &
pids+=("$!")
wait_for_ping 7379 || { echo "Ignix did not start:" >&2; cat "$workdir/ignix.log" >&2; exit 1; }

if port_in_use 6379; then
  echo "Using the Redis server already running on port 6379."
elif command -v redis-server >/dev/null 2>&1; then
  echo "Starting Redis (AOF, appendfsync everysec)..."
  mkdir -p "$workdir/redis"
  redis-server --port 6379 --bind 127.0.0.1 --dir "$workdir/redis" \
    --save "" --appendonly yes --appendfsync everysec >"$workdir/redis.log" 2>&1 &
  pids+=("$!")
  wait_for_ping 6379 || { echo "Redis did not start:" >&2; cat "$workdir/redis.log" >&2; exit 1; }
else
  echo "Redis is not running on port 6379 and redis-server is not installed." >&2
  exit 1
fi

echo "Running benchmarks..."
python3 benchmarks/run_all.py
