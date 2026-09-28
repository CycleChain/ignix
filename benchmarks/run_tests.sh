#!/usr/bin/env bash
# Run the large payload tests against a release build of Ignix started in a
# temporary directory, then stop only that server. The exit status is the
# status of the test run. Run from anywhere: bash benchmarks/run_tests.sh
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"

if lsof -nP -iTCP:7379 -sTCP:LISTEN >/dev/null 2>&1; then
  echo "Port 7379 is already in use; stop that server first (lsof -nP -iTCP:7379 -sTCP:LISTEN)." >&2
  exit 1
fi

echo "Compiling..."
cargo build --release

workdir="$(mktemp -d)"
(cd "$workdir" && exec "$repo/target/release/ignix") >"$workdir/server.log" 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true; wait "$server" 2>/dev/null || true; rm -rf "$workdir"' EXIT

python3 - <<'EOF' || { echo "Ignix did not start:" >&2; cat "$workdir/server.log" >&2; exit 1; }
import sys, time
sys.path.insert(0, "benchmarks/scripts")
from resp_client import ping
for _ in range(100):
    if ping("127.0.0.1", 7379):
        sys.exit(0)
    time.sleep(0.1)
sys.exit(1)
EOF

echo "Running tests..."
cargo test --test large_payloads -- --include-ignored --nocapture
