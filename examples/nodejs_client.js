#!/usr/bin/env node
/**
 * Ignix Node.js Client Example
 * 
 * This example demonstrates how to use Ignix with the redis npm package.
 * Ignix is fully compatible with Redis clients.
 * 
 * Installation:
 *     npm install redis
 * 
 * Usage:
 *     node examples/nodejs_client.js
 */

const redis = require('redis');

async function main() {
    console.log('🔥 Ignix Node.js Client Example');
    console.log('=' .repeat(40));
    
    let client;
    
    try {
        // Connect to Ignix server
        console.log('Connecting to Ignix server at localhost:7379...');
        client = redis.createClient({
            socket: {
                host: 'localhost',
                port: 7379,
                connectTimeout: 5000,
                // Fail instead of retrying forever when Ignix is not running
                reconnectStrategy: false
            },
            // Ignix does not implement CLIENT SETINFO
            disableClientInfo: true
        });
        
        // Handle connection events
        client.on('error', (err) => {
            console.error('❌ Redis Client Error:', err);
        });
        
        client.on('connect', () => {
            console.log('🔗 Connected to Ignix server');
        });
        
        // Connect to server
        await client.connect();
        
        // Test connection
        console.log('Testing connection...');
        const pong = await client.ping();
        console.log(`PING response: ${pong}`);
        
        console.log('\n📝 Basic Operations:');
        console.log('-'.repeat(20));
        
        // SET operation
        console.log("Setting key 'hello' to 'world'...");
        await client.set('hello', 'world');
        console.log('✅ SET hello world');
        
        // GET operation
        const value = await client.get('hello');
        console.log(`✅ GET hello: ${value}`);
        
        // EXISTS operation
        const exists = await client.exists('hello');
        console.log(`✅ EXISTS hello: ${exists}`);
        
        // SET with different data types
        await client.set('counter', '0');
        await client.set('user:1:name', 'Alice');
        await client.set('user:1:age', '25');
        
        console.log('\n🔢 Counter Operations:');
        console.log('-'.repeat(25));
        
        // INCR operation
        for (let i = 0; i < 5; i++) {
            const counter = await client.incr('counter');
            console.log(`✅ INCR counter: ${counter}`);
            await sleep(100); // Sleep 100ms
        }
        
        console.log('\n👤 User Data:');
        console.log('-'.repeat(15));
        
        // Multiple GET operations
        const name = await client.get('user:1:name');
        const age = await client.get('user:1:age');
        console.log(`✅ User: ${name}, Age: ${age}`);
        
        console.log('\n🗂️  Bulk Operations:');
        console.log('-'.repeat(20));
        
        // MSET - Multiple SET
        await client.mSet([
            'fruit:1', 'apple',
            'fruit:2', 'banana', 
            'fruit:3', 'orange'
        ]);
        console.log('✅ MSET fruit:1=apple, fruit:2=banana, fruit:3=orange');
        
        // MGET - Multiple GET
        const fruits = await client.mGet(['fruit:1', 'fruit:2', 'fruit:3']);
        console.log(`✅ MGET fruits: ${fruits}`);
        
        console.log('\n🔄 Key Management:');
        console.log('-'.repeat(20));
        
        // RENAME operation
        await client.rename('hello', 'greeting');
        console.log('✅ RENAME hello -> greeting');
        
        // Verify rename
        const oldExists = await client.exists('hello');
        const newExists = await client.exists('greeting');
        const newValue = await client.get('greeting');
        console.log(`✅ hello exists: ${oldExists}, greeting exists: ${newExists}, value: ${newValue}`);
        
        // DEL operation
        const deleted = await client.del('greeting');
        console.log(`✅ DEL greeting: ${deleted} key(s) deleted`);
        
        console.log('\n📊 Statistics:');
        console.log('-'.repeat(15));
        
        // Count the keys this example created (Ignix does not implement KEYS)
        const knownKeys = ['counter', 'user:1:name', 'user:1:age',
                           'fruit:1', 'fruit:2', 'fruit:3', 'greeting'];
        const existing = await client.exists(knownKeys);
        console.log(`✅ ${existing} of ${knownKeys.length} example keys exist`);
        
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
        // Close connection
        // Ignix does not implement QUIT, so close the socket directly
        if (client && client.isOpen) {
            await client.disconnect();
            console.log('\n🔌 Disconnected from Ignix server');
        }
    }
}

// Helper function to sleep
function sleep(ms) {
    return new Promise(resolve => setTimeout(resolve, ms));
}

// Handle unhandled promise rejections
process.on('unhandledRejection', (reason, promise) => {
    console.error('Unhandled Rejection at:', promise, 'reason:', reason);
    process.exit(1);
});

// Run the example
main().catch(console.error);
