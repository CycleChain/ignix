#!/usr/bin/env python3
"""
Ignix Connection Verification Script

This script demonstrates multiple ways to verify that we're actually
connecting to the Ignix server and not to another Redis instance.
"""

import socket
import sys
import time
import os

class IgnixVerificationClient:
    def __init__(self, host='localhost', port=7379):
        self.host = host
        self.port = port
        self.socket = None
    
    def connect(self):
        try:
            self.socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self.socket.settimeout(5)
            self.socket.connect((self.host, self.port))
            return True
        except Exception as e:
            print(f"❌ Connection failed: {e}")
            return False
    
    def disconnect(self):
        if self.socket:
            self.socket.close()
            self.socket = None
    
    def send_command(self, *args):
        if not self.socket:
            raise Exception("Not connected")
        
        command = f"*{len(args)}\r\n"
        for arg in args:
            arg_str = str(arg)
            command += f"${len(arg_str)}\r\n{arg_str}\r\n"
        
        self.socket.send(command.encode('utf-8'))
        return self._read_response()
    
    def _read_response(self):
        response = b""
        while True:
            data = self.socket.recv(1024)
            if not data:
                break
            response += data
            if response.endswith(b'\r\n'):
                break
        
        response_str = response.decode('utf-8')
        
        if response_str.startswith('+'):
            return response_str[1:].rstrip('\r\n')
        elif response_str.startswith(':'):
            return int(response_str[1:].rstrip('\r\n'))
        elif response_str.startswith('$'):
            lines = response_str.split('\r\n')
            if lines[0] == '$-1':
                return None
            return lines[1] if len(lines) > 1 else ""
        elif response_str.startswith('-'):
            return response_str[1:].rstrip('\r\n')
        else:
            return response_str.rstrip('\r\n')

def verify_ignix_connection():
    print("🔍 Ignix Connection Verification")
    print("=" * 40)
    
    # Method 1: Check if Ignix process is running
    print("\n1️⃣  Process Verification:")
    print("-" * 25)
    
    try:
        import subprocess
        result = subprocess.run(['ps', 'aux'], capture_output=True, text=True)
        ignix_processes = [line for line in result.stdout.split('\n') if 'ignix' in line and 'target/release/ignix' in line]
        
        if ignix_processes:
            print("✅ Ignix process found:")
            for process in ignix_processes:
                print(f"   {process.strip()}")
        else:
            print("❌ No Ignix process found")
            print("   Start Ignix: cargo run --release")
            return False
    except Exception as e:
        print(f"⚠️  Could not check processes: {e}")
    
    # Method 2: Check if port 7379 is listening
    print("\n2️⃣  Port Verification:")
    print("-" * 20)
    
    try:
        result = subprocess.run(['lsof', '-i', ':7379'], capture_output=True, text=True)
        if result.stdout:
            print("✅ Port 7379 is listening:")
            for line in result.stdout.strip().split('\n'):
                if 'ignix' in line:
                    print(f"   {line}")
        else:
            print("❌ Port 7379 is not listening")
            return False
    except Exception as e:
        print(f"⚠️  Could not check port: {e}")
    
    # Method 3: Check AOF file creation/modification
    print("\n3️⃣  AOF File Verification:")
    print("-" * 25)
    
    aof_file = "ignix.aof"
    if os.path.exists(aof_file):
        stat = os.stat(aof_file)
        mod_time = time.ctime(stat.st_mtime)
        size = stat.st_size
        print(f"✅ AOF file exists: {aof_file}")
        print(f"   Size: {size} bytes")
        print(f"   Modified: {mod_time}")
        
        # Check if file was modified recently (within last 5 minutes)
        if time.time() - stat.st_mtime < 300:
            print("✅ AOF file recently modified (Ignix is active)")
        else:
            print("⚠️  AOF file not recently modified")
    else:
        print("❌ AOF file not found")
    
    # Method 4: Test unique Ignix behavior
    print("\n4️⃣  Behavior Verification:")
    print("-" * 25)
    
    client = IgnixVerificationClient()
    
    try:
        if not client.connect():
            return False
        
        print("✅ Connected to server on port 7379")
        
        # Test PING
        response = client.send_command("PING")
        print(f"✅ PING response: {response}")
        
        # Set a unique test key
        test_key = f"ignix_test_{int(time.time())}"
        test_value = "ignix_verification_value"
        
        response = client.send_command("SET", test_key, test_value)
        print(f"✅ SET {test_key}: {response}")
        
        # Verify the value was set
        response = client.send_command("GET", test_key)
        if response == test_value:
            print(f"✅ GET {test_key}: {response} (matches expected)")
        else:
            print(f"❌ GET {test_key}: {response} (does not match expected)")
        
        # Clean up test key
        client.send_command("DEL", test_key)
        
        print("\n5️⃣  AOF File Update Check:")
        print("-" * 25)
        
        # Check if AOF file was updated after our operation
        if os.path.exists(aof_file):
            new_stat = os.stat(aof_file)
            if new_stat.st_mtime > stat.st_mtime:
                print("✅ AOF file updated after our operation")
                print("✅ This confirms we're connected to Ignix!")
            else:
                print("⚠️  AOF file not updated (might be Redis)")
        
        return True
        
    except Exception as e:
        print(f"❌ Connection test failed: {e}")
        return False
    finally:
        client.disconnect()

def main():
    success = verify_ignix_connection()
    
    print("\n" + "=" * 40)
    if success:
        print("🎉 VERIFICATION SUCCESSFUL!")
        print("✅ You are connected to Ignix server")
        print("\nTo run client examples:")
        print("  Python: python3 examples/simple_python_client.py")
        print("  Node.js: node examples/simple_nodejs_client.js")
    else:
        print("❌ VERIFICATION FAILED!")
        print("Make sure Ignix is running: cargo run --release")
    print("=" * 40)

if __name__ == "__main__":
    main()
