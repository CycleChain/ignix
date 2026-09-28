#!/usr/bin/env node
/**
 * Simple Ignix Node.js Client Example
 * 
 * This example demonstrates basic operations with Ignix using raw TCP sockets
 * and the RESP protocol. This avoids potential compatibility issues with 
 * redis npm package features that aren't implemented in Ignix yet.
 * 
 * Usage:
 *     node examples/simple_nodejs_client.js
 */

const net = require('net');

class SimpleRedisClient {
    constructor(host = 'localhost', port = 7379) {
        this.host = host;
        this.port = port;
        this.socket = null;
        this.connected = false;
        this.buffer = Buffer.alloc(0);
        this.pending = [];
    }
    
    connect() {
        return new Promise((resolve, reject) => {
            this.socket = new net.Socket();
            this.socket.setTimeout(5000);
            
            this.socket.on('connect', () => {
                this.connected = true;
                resolve(true);
            });
            
            // Replies can arrive split across several chunks or several in one
            // chunk, so keep the unread bytes and answer requests in order
            this.socket.on('data', (data) => {
                this.buffer = Buffer.concat([this.buffer, data]);
                this._drain();
            });
            
            this.socket.on('error', (err) => {
                this._failPending(err);
                reject(err);
            });
            
            this.socket.on('timeout', () => {
                const err = new Error('Connection timeout');
                this._failPending(err);
                reject(err);
            });
            
            this.socket.on('close', () => {
                this.connected = false;
                this._failPending(new Error('Connection closed by server'));
            });
            
            this.socket.connect(this.port, this.host);
        });
    }
    
    disconnect() {
        if (this.socket) {
            this.socket.destroy();
            this.socket = null;
            this.connected = false;
        }
    }
    
    sendCommand(...args) {
        return new Promise((resolve, reject) => {
            if (!this.connected) {
                reject(new Error('Not connected'));
                return;
            }
            
            // Build RESP command; bulk lengths count bytes, not characters
            const parts = [Buffer.from(`*${args.length}\r\n`)];
            for (const arg of args) {
                const bytes = Buffer.from(String(arg));
                parts.push(Buffer.from(`$${bytes.length}\r\n`), bytes, Buffer.from('\r\n'));
            }
            
            this.pending.push({ resolve, reject });
            this.socket.write(Buffer.concat(parts));
        });
    }
    
    _drain() {
        while (this.pending.length > 0) {
            const reply = this._parseReply(this.buffer, 0);
            if (reply === null) {
                return; // The reply is not complete yet
            }
            this.buffer = this.buffer.subarray(reply.end);
            const { resolve, reject } = this.pending.shift();
            if (reply.value instanceof Error) {
                reject(reply.value);
            } else {
                resolve(reply.value);
            }
        }
    }
    
    _failPending(err) {
        for (const { reject } of this.pending.splice(0)) {
            reject(err);
        }
    }
    
    // Parse one RESP reply starting at `pos`. Returns { value, end }, or null
    // when the buffer does not hold the whole reply yet.
    _parseReply(buf, pos) {
        const lineEnd = buf.indexOf('\r\n', pos);
        if (lineEnd === -1) {
            return null;
        }
        const type = String.fromCharCode(buf[pos]);
        const line = buf.toString('utf8', pos + 1, lineEnd);
        const next = lineEnd + 2;
        
        if (type === '+') {
            // Simple string
            return { value: line, end: next };
        } else if (type === '-') {
            // Error
            return { value: new Error(line), end: next };
        } else if (type === ':') {
            // Integer
            return { value: parseInt(line, 10), end: next };
        } else if (type === '$') {
            // Bulk string; $-1 is null
            const length = parseInt(line, 10);
            if (length < 0) {
                return { value: null, end: next };
            }
            if (buf.length < next + length + 2) {
                return null;
            }
            return { value: buf.toString('utf8', next, next + length), end: next + length + 2 };
        } else if (type === '*') {
            // Array
            const count = parseInt(line, 10);
            if (count < 0) {
                return { value: null, end: next };
            }
            const items = [];
            let end = next;
            for (let i = 0; i < count; i++) {
                const item = this._parseReply(buf, end);
                if (item === null) {
                    return null;
                }
                items.push(item.value);
                end = item.end;
            }
            return { value: items, end };
        }
        return { value: new Error(`Unexpected reply: ${line}`), end: next };
    }
}

async function main() {
    console.log('🔥 Simple Ignix Node.js Client Example');
    console.log('=' .repeat(45));
    
    const client = new SimpleRedisClient();
    
    try {
        // Connect to server
        console.log('Connecting to Ignix server at localhost:7379...');
        await client.connect();
        console.log('✅ Connected successfully!');
        
        // Test PING
        console.log('\n🏓 Testing Connection:');
        console.log('-'.repeat(20));
        const pingResponse = await client.sendCommand('PING');
        console.log(`PING response: ${pingResponse}`);
        
        console.log('\n📝 Basic Operations:');
        console.log('-'.repeat(20));
        
        // SET operation
        const setResponse = await client.sendCommand('SET', 'hello', 'world');
        console.log(`✅ SET hello world: ${setResponse}`);
        
        // GET operation
        const getResponse = await client.sendCommand('GET', 'hello');
        console.log(`✅ GET hello: ${getResponse}`);
        
        // EXISTS operation
        const existsResponse = await client.sendCommand('EXISTS', 'hello');
        console.log(`✅ EXISTS hello: ${existsResponse}`);
        
        console.log('\n🔢 Counter Operations:');
        console.log('-'.repeat(25));
        
        // SET counter to 0
        await client.sendCommand('SET', 'counter', '0');
        
        // INCR operations
        for (let i = 0; i < 3; i++) {
            const incrResponse = await client.sendCommand('INCR', 'counter');
            console.log(`✅ INCR counter: ${incrResponse}`);
        }
        
        console.log('\n🗂️  Multiple Operations:');
        console.log('-'.repeat(25));
        
        // MSET operation
        const msetResponse = await client.sendCommand('MSET', 'fruit1', 'apple', 'fruit2', 'banana');
        console.log(`✅ MSET fruit1=apple fruit2=banana: ${msetResponse}`);
        
        // MGET operation
        const mgetResponse = await client.sendCommand('MGET', 'fruit1', 'fruit2');
        console.log(`✅ MGET fruit1 fruit2: ${JSON.stringify(mgetResponse)}`);
        
        console.log('\n🔄 Key Management:');
        console.log('-'.repeat(20));
        
        // RENAME operation
        const renameResponse = await client.sendCommand('RENAME', 'hello', 'greeting');
        console.log(`✅ RENAME hello -> greeting: ${renameResponse}`);
        
        // Verify the rename worked
        const greetingResponse = await client.sendCommand('GET', 'greeting');
        console.log(`✅ GET greeting: ${greetingResponse}`);
        
        // EXISTS check on old key
        const oldExistsResponse = await client.sendCommand('EXISTS', 'hello');
        console.log(`✅ EXISTS hello (should be 0): ${oldExistsResponse}`);
        
        // DEL operation
        const delResponse = await client.sendCommand('DEL', 'greeting');
        console.log(`✅ DEL greeting: ${delResponse}`);
        
        console.log('\n✅ All operations completed successfully!');
        
    } catch (error) {
        if (error.code === 'ECONNREFUSED') {
            console.error('❌ Connection Error: Could not connect to Ignix server');
            console.error('Make sure Ignix server is running: cargo run --release');
        } else {
            console.error('❌ Error:', error.message);
        }
        process.exit(1);
    } finally {
        client.disconnect();
        console.log('\n🔌 Disconnected from server');
    }
}

// Handle unhandled promise rejections
process.on('unhandledRejection', (reason, promise) => {
    console.error('Unhandled Rejection at:', promise, 'reason:', reason);
    process.exit(1);
});

// Run the example
main().catch(console.error);
