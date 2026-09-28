# Ignix Client Examples

This directory contains client examples demonstrating how to connect to and use Ignix with various programming languages.

## Available Examples

### 1. Rust Client (`client.rs`)
Basic Rust client using raw TCP sockets and RESP protocol.

```bash
cargo run --example client
```

### 2. Python Client (`python_client.py`)
Python client using the `redis-py` library.

```bash
# Install dependencies
pip install -r examples/requirements.txt

# Run example
python3 examples/python_client.py
```

### 3. Node.js Client (`nodejs_client.js`)
Node.js client using the `redis` npm package.

```bash
# Install dependencies
cd examples && npm install

# Run example
node nodejs_client.js
```

## Prerequisites

1. **Start Ignix Server**
   ```bash
   cargo run --release
   ```
   The server will start on `localhost:7379`

2. **Install Client Dependencies** (for Python/Node.js examples, see above)

## Supported Operations

All examples demonstrate these Redis-compatible operations:

- `PING` - Test server connectivity
- `SET key value` - Set a key-value pair
- `GET key` - Retrieve a value by key
- `EXISTS key [key ...]` - Count how many keys exist
- `INCR key` - Increment a numeric value (redis-py sends `INCRBY key 1`)
- `DEL key [key ...]` - Delete keys
- `RENAME oldkey newkey` - Rename a key
- `MSET key1 value1 key2 value2` - Set multiple keys
- `MGET key1 key2 key3` - Get multiple values

## Example Output

When running successfully, you should see output similar to:

```
🔥 Ignix Python Client Example
========================================
Connecting to Ignix server at localhost:7379...
Testing connection...
PING response: True

📝 Basic Operations:
--------------------
Setting key 'hello' to 'world'...
✅ SET hello world
✅ GET hello: world
✅ EXISTS hello: 1

🔢 Counter Operations:
-------------------------
✅ INCR counter: 1
✅ INCR counter: 2
✅ INCR counter: 3
✅ INCR counter: 4
✅ INCR counter: 5

✅ All operations completed successfully!
```

## Troubleshooting

### Connection Refused
```
❌ Connection Error: Could not connect to Ignix server
```
**Solution**: Make sure the Ignix server is running:
```bash
cargo run --release
```

### Command Not Found
```
❌ Redis Error: unknown command 'KEYS', with args beginning with: '*'
```
**Solution**: The command is not implemented in Ignix yet. Check the [supported commands list](../README.md#-supported-commands).

### Python Issues
If you get import errors:
```bash
pip install "redis>=5.0.0"
```

### Node.js Issues
If you get module not found errors:
```bash
cd examples
npm install redis
```

## Writing Your Own Client

Ignix implements the Redis Serialization Protocol (RESP), so any Redis client library should work. Here's a minimal example:

### Python (redis-py)
```python
import redis
client = redis.Redis(host='localhost', port=7379, decode_responses=True)
client.set('key', 'value')
print(client.get('key'))
```

### Node.js (redis)
```javascript
const redis = require('redis');
const client = redis.createClient({ socket: { host: 'localhost', port: 7379 } });
await client.connect();
await client.set('key', 'value');
console.log(await client.get('key'));
```

### Raw RESP Protocol
Any Redis tool can talk to Ignix, for example `redis-cli`:
```bash
redis-cli -p 7379 SET key value
redis-cli -p 7379 GET key
```

## Performance Testing

Micro-benchmarks and the Redis comparison live in `benches/` and `benchmarks/`:
```bash
cargo bench --bench exec --bench resp
bash benchmarks/run_benchmarks.sh
```
See [benchmarks/BENCHMARK_README.md](../benchmarks/BENCHMARK_README.md).
