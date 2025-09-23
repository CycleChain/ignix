#!/usr/bin/env python3
"""
Ignix Python Client Example

This example demonstrates how to use Ignix with the redis-py library.
Ignix is fully compatible with Redis clients.

Installation:
    pip install redis

Usage:
    python examples/python_client.py
"""

import redis
import time
import sys

def main():
    print("🔥 Ignix Python Client Example")
    print("=" * 40)
    
    try:
        # Connect to Ignix server
        print("Connecting to Ignix server at localhost:7379...")
        client = redis.Redis(
            host='localhost',
            port=7379,
            decode_responses=True,  # Automatically decode bytes to strings
            socket_connect_timeout=5,
            socket_timeout=5
        )
        
        # Test connection
        print("Testing connection...")
        response = client.ping()
        print(f"PING response: {response}")
        
        print("\n📝 Basic Operations:")
        print("-" * 20)
        
        # SET operation
        print("Setting key 'hello' to 'world'...")
        client.set('hello', 'world')
        print("✅ SET hello world")
        
        # GET operation
        value = client.get('hello')
        print(f"✅ GET hello: {value}")
        
        # EXISTS operation
        exists = client.exists('hello')
        print(f"✅ EXISTS hello: {exists}")
        
        # SET with different data types
        client.set('counter', 0)
        client.set('user:1:name', 'Alice')
        client.set('user:1:age', 25)
        
        print("\n🔢 Counter Operations:")
        print("-" * 25)
        
        # INCR operation
        for i in range(5):
            counter = client.incr('counter')
            print(f"✅ INCR counter: {counter}")
            time.sleep(0.1)
        
        print("\n👤 User Data:")
        print("-" * 15)
        
        # Multiple GET operations
        name = client.get('user:1:name')
        age = client.get('user:1:age')
        print(f"✅ User: {name}, Age: {age}")
        
        print("\n🗂️  Bulk Operations:")
        print("-" * 20)
        
        # MSET - Multiple SET
        client.mset({
            'fruit:1': 'apple',
            'fruit:2': 'banana',
            'fruit:3': 'orange'
        })
        print("✅ MSET fruit:1=apple, fruit:2=banana, fruit:3=orange")
        
        # MGET - Multiple GET
        fruits = client.mget(['fruit:1', 'fruit:2', 'fruit:3'])
        print(f"✅ MGET fruits: {fruits}")
        
        print("\n🔄 Key Management:")
        print("-" * 20)
        
        # RENAME operation
        client.rename('hello', 'greeting')
        print("✅ RENAME hello -> greeting")
        
        # Verify rename
        old_exists = client.exists('hello')
        new_exists = client.exists('greeting')
        new_value = client.get('greeting')
        print(f"✅ hello exists: {old_exists}, greeting exists: {new_exists}, value: {new_value}")
        
        # DEL operation
        deleted = client.delete('greeting')
        print(f"✅ DEL greeting: {deleted} key(s) deleted")
        
        print("\n📊 Statistics:")
        print("-" * 15)
        
        # Count remaining keys
        all_keys = client.keys('*')
        print(f"✅ Total keys: {len(all_keys)}")
        print(f"✅ Keys: {all_keys}")
        
        print("\n✅ All operations completed successfully!")
        
    except redis.ConnectionError as e:
        print(f"❌ Connection Error: {e}")
        print("Make sure Ignix server is running: cargo run --release")
        sys.exit(1)
    except redis.RedisError as e:
        print(f"❌ Redis Error: {e}")
        sys.exit(1)
    except Exception as e:
        print(f"❌ Unexpected Error: {e}")
        sys.exit(1)

if __name__ == "__main__":
    main()
