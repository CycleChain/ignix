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
 *     IGNIX_PASSWORD=secret node examples/nodejs_client.js   # ignix --requirepass secret
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
            // Sent with HELLO or AUTH when the server runs with --requirepass
            password: process.env.IGNIX_PASSWORD,
            socket: {
                host: 'localhost',
                port: 7379,
                connectTimeout: 5000,
                // Fail instead of retrying forever when Ignix is not running
                reconnectStrategy: false
            }
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
        
        console.log('\n⏳ Expiry:');
        console.log('-'.repeat(10));
        
        // SET with an expiry, then read and remove it
        await client.set('session:1', 'token', { EX: 60 });
        console.log(`✅ SET session:1 EX 60, TTL: ${await client.ttl('session:1')} s`);
        await client.persist('session:1');
        console.log(`✅ PERSIST session:1, TTL: ${await client.ttl('session:1')} (no expiry)`);
        await client.set('session:1', 'token', { PX: 100 });
        await sleep(200);
        console.log(`✅ After PX 100 and 200 ms: session:1 = ${await client.get('session:1')}`);
        
        console.log('\n📊 Statistics:');
        console.log('-'.repeat(15));
        
        // The keys this example created, found with SCAN
        const found = [];
        for await (const key of client.scanIterator({ MATCH: '*:*' })) {
            found.push(key);
        }
        found.sort();
        console.log(`✅ SCAN MATCH *:* found ${found.length} keys: ${found.join(', ')}`);
        console.log(`✅ DBSIZE: ${await client.dbSize()}`);
        
        console.log('\n✅ All operations completed successfully!');
        
    } catch (error) {
        if (error.code === 'ECONNREFUSED') {
            console.error('❌ Connection Error: Could not connect to Ignix server');
            console.error('Make sure Ignix server is running: cargo run --release');
        } else if (/^(NOAUTH|WRONGPASS)/.test(error.message)) {
            console.error('❌ Authentication Error:', error.message);
            console.error('The server runs with --requirepass: set IGNIX_PASSWORD to its password');
        } else {
            console.error('❌ Error:', error.message);
        }
        process.exit(1);
    } finally {
        // Close connection
        if (client && client.isOpen) {
            await client.quit();
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
