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
    }
    
    connect() {
        return new Promise((resolve, reject) => {
            this.socket = new net.Socket();
            this.socket.setTimeout(5000);
            
            this.socket.on('connect', () => {
                this.connected = true;
                resolve(true);
            });
            
            this.socket.on('error', (err) => {
                reject(err);
            });
            
            this.socket.on('timeout', () => {
                reject(new Error('Connection timeout'));
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
            
            // Build RESP command
            let command = `*${args.length}\r\n`;
            for (const arg of args) {
                const argStr = String(arg);
                command += `$${argStr.length}\r\n${argStr}\r\n`;
            }
            
            // Set up response handler
            let responseData = '';
            const responseHandler = (data) => {
                responseData += data.toString();
                if (responseData.includes('\r\n')) {
                    this.socket.removeListener('data', responseHandler);
                    resolve(this._parseResponse(responseData));
                }
            };
            
            this.socket.on('data', responseHandler);
            
            // Send command
            this.socket.write(command);
        });
    }
    
    _parseResponse(responseStr) {
        const trimmed = responseStr.trim();
        
        if (trimmed.startsWith('+')) {
            // Simple string
            return trimmed.substring(1);
        } else if (trimmed.startsWith(':')) {
            // Integer
            return parseInt(trimmed.substring(1));
        } else if (trimmed.startsWith('$')) {
            // Bulk string
            const lines = trimmed.split('\r\n');
            if (lines[0] === '$-1') {
                return null; // Null
            }
            const length = parseInt(lines[0].substring(1));
            return lines[1] || '';
        } else if (trimmed.startsWith('-')) {
            // Error
            return trimmed.substring(1);
        } else if (trimmed.startsWith('*')) {
            // Array - for simplicity, return raw response
            return trimmed;
        } else {
            return trimmed;
        }
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
        console.log(`✅ MGET fruit1 fruit2: ${mgetResponse}`);
        
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
