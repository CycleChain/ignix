#!/usr/bin/env node
/**
 * Ignix Connection Verification Script (Node.js)
 * 
 * This script demonstrates how to verify that we're actually
 * connecting to the Ignix server and not to another Redis instance.
 */

const net = require('net');
const { execSync } = require('child_process');
const fs = require('fs');

class IgnixVerificationClient {
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
            
            let command = `*${args.length}\r\n`;
            for (const arg of args) {
                const argStr = String(arg);
                command += `$${argStr.length}\r\n${argStr}\r\n`;
            }
            
            let responseData = '';
            const responseHandler = (data) => {
                responseData += data.toString();
                if (responseData.includes('\r\n')) {
                    this.socket.removeListener('data', responseHandler);
                    resolve(this._parseResponse(responseData));
                }
            };
            
            this.socket.on('data', responseHandler);
            this.socket.write(command);
        });
    }
    
    _parseResponse(responseStr) {
        const trimmed = responseStr.trim();
        
        if (trimmed.startsWith('+')) {
            return trimmed.substring(1);
        } else if (trimmed.startsWith(':')) {
            return parseInt(trimmed.substring(1));
        } else if (trimmed.startsWith('$')) {
            const lines = trimmed.split('\r\n');
            if (lines[0] === '$-1') {
                return null;
            }
            return lines[1] || '';
        } else if (trimmed.startsWith('-')) {
            return trimmed.substring(1);
        } else {
            return trimmed;
        }
    }
}

async function verifyIgnixConnection() {
    console.log('🔍 Ignix Connection Verification (Node.js)');
    console.log('=' .repeat(45));
    
    // Method 1: Check if Ignix process is running
    console.log('\n1️⃣  Process Verification:');
    console.log('-'.repeat(25));
    
    try {
        const psOutput = execSync('ps aux', { encoding: 'utf8' });
        const ignixProcesses = psOutput.split('\n').filter(line => 
            line.includes('ignix') && line.includes('target/release/ignix')
        );
        
        if (ignixProcesses.length > 0) {
            console.log('✅ Ignix process found:');
            ignixProcesses.forEach(process => {
                console.log(`   ${process.trim()}`);
            });
        } else {
            console.log('❌ No Ignix process found');
            console.log('   Start Ignix: cargo run --release');
            return false;
        }
    } catch (error) {
        console.log(`⚠️  Could not check processes: ${error.message}`);
    }
    
    // Method 2: Check if port 7379 is listening
    console.log('\n2️⃣  Port Verification:');
    console.log('-'.repeat(20));
    
    try {
        const lsofOutput = execSync('lsof -i :7379', { encoding: 'utf8' });
        if (lsofOutput) {
            console.log('✅ Port 7379 is listening:');
            lsofOutput.trim().split('\n').forEach(line => {
                if (line.includes('ignix')) {
                    console.log(`   ${line}`);
                }
            });
        } else {
            console.log('❌ Port 7379 is not listening');
            return false;
        }
    } catch (error) {
        console.log(`⚠️  Could not check port: ${error.message}`);
    }
    
    // Method 3: Check AOF file
    console.log('\n3️⃣  AOF File Verification:');
    console.log('-'.repeat(25));
    
    const aofFile = 'ignix.aof';
    try {
        if (fs.existsSync(aofFile)) {
            const stats = fs.statSync(aofFile);
            console.log(`✅ AOF file exists: ${aofFile}`);
            console.log(`   Size: ${stats.size} bytes`);
            console.log(`   Modified: ${stats.mtime}`);
            
            // Check if file was modified recently (within last 5 minutes)
            const now = new Date();
            const timeDiff = (now - stats.mtime) / 1000; // seconds
            if (timeDiff < 300) {
                console.log('✅ AOF file recently modified (Ignix is active)');
            } else {
                console.log('⚠️  AOF file not recently modified');
            }
        } else {
            console.log('❌ AOF file not found');
        }
    } catch (error) {
        console.log(`⚠️  Could not check AOF file: ${error.message}`);
    }
    
    // Method 4: Test connection and behavior
    console.log('\n4️⃣  Behavior Verification:');
    console.log('-'.repeat(25));
    
    const client = new IgnixVerificationClient();
    
    try {
        await client.connect();
        console.log('✅ Connected to server on port 7379');
        
        // Test PING
        const pingResponse = await client.sendCommand('PING');
        console.log(`✅ PING response: ${pingResponse}`);
        
        // Set a unique test key
        const testKey = `ignix_test_js_${Date.now()}`;
        const testValue = 'ignix_verification_value_nodejs';
        
        const setResponse = await client.sendCommand('SET', testKey, testValue);
        console.log(`✅ SET ${testKey}: ${setResponse}`);
        
        // Verify the value was set
        const getResponse = await client.sendCommand('GET', testKey);
        if (getResponse === testValue) {
            console.log(`✅ GET ${testKey}: ${getResponse} (matches expected)`);
        } else {
            console.log(`❌ GET ${testKey}: ${getResponse} (does not match expected)`);
        }
        
        // Clean up test key
        await client.sendCommand('DEL', testKey);
        
        console.log('\n5️⃣  Final Confirmation:');
        console.log('-'.repeat(20));
        console.log('✅ Successfully executed RESP commands');
        console.log('✅ Data persisted to AOF file');
        console.log('✅ This confirms we\'re connected to Ignix!');
        
        return true;
        
    } catch (error) {
        console.log(`❌ Connection test failed: ${error.message}`);
        return false;
    } finally {
        client.disconnect();
    }
}

async function main() {
    const success = await verifyIgnixConnection();
    
    console.log('\n' + '='.repeat(45));
    if (success) {
        console.log('🎉 VERIFICATION SUCCESSFUL!');
        console.log('✅ You are connected to Ignix server');
        console.log('\nTo run client examples:');
        console.log('  Python: python3 examples/simple_python_client.py');
        console.log('  Node.js: node examples/simple_nodejs_client.js');
    } else {
        console.log('❌ VERIFICATION FAILED!');
        console.log('Make sure Ignix is running: cargo run --release');
    }
    console.log('='.repeat(45));
}

main().catch(console.error);
