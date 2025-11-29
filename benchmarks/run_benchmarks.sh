#!/bin/bash
echo "Compiling..."
cd ..
cargo build --release
if [ $? -ne 0 ]; then
    echo "Compilation failed"
    exit 1
fi

echo "Starting Ignix..."
pkill -9 ignix
nohup ./target/release/ignix > benchmarks/server.log 2>&1 &
SERVER_PID=$!
sleep 2

echo "Running benchmarks..."
cd benchmarks
python run_all.py

kill $SERVER_PID
