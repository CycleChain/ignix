#!/bin/bash
echo "Starting test run..." > test_status.txt
pkill -9 ignix

# Compile first
echo "Compiling..." >> test_status.txt
cargo build --release
if [ $? -ne 0 ]; then
    echo "Compilation failed" >> test_status.txt
    exit 1
fi

# Start server
echo "Starting server..." >> test_status.txt
nohup ./target/release/ignix > server.log 2>&1 &
SERVER_PID=$!
echo "Server started with PID $SERVER_PID" >> test_status.txt

# Wait for server to be ready
sleep 5

echo "Running tests..." >> test_status.txt
cargo test --test large_payloads -- --nocapture > test_output.txt 2>&1
EXIT_CODE=$?
echo "Tests finished with exit code $EXIT_CODE" >> test_status.txt
kill $SERVER_PID
