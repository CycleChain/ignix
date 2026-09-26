#!/usr/bin/env python3
"""
Simple Ignix Python Client Example

This example demonstrates basic operations with Ignix using raw TCP sockets
and the RESP protocol. This avoids potential compatibility issues with 
redis-py client library features that aren't implemented in Ignix yet.

Usage:
    python3 examples/simple_python_client.py
"""

import socket
import sys

class ResponseError(Exception):
    """Error reply from the server"""

class SimpleRedisClient:
    def __init__(self, host='localhost', port=7379):
        self.host = host
        self.port = port
        self.socket = None
        self.reader = None
    
    def connect(self):
        """Connect to the Ignix server"""
        try:
            self.socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self.socket.settimeout(5)
            self.socket.connect((self.host, self.port))
            # Buffered reader: a reply can span several reads, and bytes after
            # one reply belong to the next
            self.reader = self.socket.makefile('rb')
            return True
        except Exception as e:
            print(f"❌ Connection failed: {e}")
            return False
    
    def disconnect(self):
        """Disconnect from the server"""
        if self.socket:
            self.reader.close()
            self.socket.close()
            self.socket = None
            self.reader = None
    
    def send_command(self, *args):
        """Send a RESP command and return the response"""
        if not self.socket:
            raise Exception("Not connected")
        
        # Build RESP command; bulk lengths count bytes, not characters
        command = b"*%d\r\n" % len(args)
        for arg in args:
            data = str(arg).encode('utf-8')
            command += b"$%d\r\n%s\r\n" % (len(data), data)
        
        # Send command
        self.socket.sendall(command)
        
        # Read response
        return self._read_response()
    
    def _read_response(self):
        """Read and parse one RESP response"""
        line = self.reader.readline()
        if not line.endswith(b'\r\n'):
            raise Exception("Connection closed by server")
        kind, rest = line[:1], line[1:-2].decode('utf-8')
        
        if kind == b'+':
            # Simple string
            return rest
        elif kind == b':':
            # Integer
            return int(rest)
        elif kind == b'$':
            # Bulk string; $-1 is null
            length = int(rest)
            if length < 0:
                return None
            data = self.reader.read(length + 2)
            if len(data) != length + 2:
                raise Exception("Connection closed by server")
            return data[:-2].decode('utf-8')
        elif kind == b'*':
            # Array
            count = int(rest)
            if count < 0:
                return None
            return [self._read_response() for _ in range(count)]
        elif kind == b'-':
            # Error
            raise ResponseError(rest)
        else:
            raise Exception(f"Unexpected reply: {line!r}")

def main():
    print("🔥 Simple Ignix Python Client Example")
    print("=" * 45)
    
    client = SimpleRedisClient()
    
    try:
        # Connect to server
        print("Connecting to Ignix server at localhost:7379...")
        if not client.connect():
            print("Make sure Ignix server is running: cargo run --release")
            sys.exit(1)
        
        print("✅ Connected successfully!")
        
        # Test PING
        print("\n🏓 Testing Connection:")
        print("-" * 20)
        response = client.send_command("PING")
        print(f"PING response: {response}")
        
        print("\n📝 Basic Operations:")
        print("-" * 20)
        
        # SET operation
        response = client.send_command("SET", "hello", "world")
        print(f"✅ SET hello world: {response}")
        
        # GET operation
        response = client.send_command("GET", "hello")
        print(f"✅ GET hello: {response}")
        
        # EXISTS operation
        response = client.send_command("EXISTS", "hello")
        print(f"✅ EXISTS hello: {response}")
        
        print("\n🔢 Counter Operations:")
        print("-" * 25)
        
        # SET counter to 0
        client.send_command("SET", "counter", "0")
        
        # INCR operations
        for i in range(3):
            response = client.send_command("INCR", "counter")
            print(f"✅ INCR counter: {response}")
        
        print("\n🗂️  Multiple Operations:")
        print("-" * 25)
        
        # MSET operation
        response = client.send_command("MSET", "fruit1", "apple", "fruit2", "banana")
        print(f"✅ MSET fruit1=apple fruit2=banana: {response}")
        
        # MGET operation
        response = client.send_command("MGET", "fruit1", "fruit2")
        print(f"✅ MGET fruit1 fruit2: {response}")
        
        print("\n🔄 Key Management:")
        print("-" * 20)
        
        # RENAME operation
        response = client.send_command("RENAME", "hello", "greeting")
        print(f"✅ RENAME hello -> greeting: {response}")
        
        # Verify the rename worked
        response = client.send_command("GET", "greeting")
        print(f"✅ GET greeting: {response}")
        
        # EXISTS check on old key
        response = client.send_command("EXISTS", "hello")
        print(f"✅ EXISTS hello (should be 0): {response}")
        
        # DEL operation
        response = client.send_command("DEL", "greeting")
        print(f"✅ DEL greeting: {response}")
        
        print("\n✅ All operations completed successfully!")
        
    except Exception as e:
        print(f"❌ Error: {e}")
        sys.exit(1)
    finally:
        client.disconnect()
        print("\n🔌 Disconnected from server")

if __name__ == "__main__":
    main()
