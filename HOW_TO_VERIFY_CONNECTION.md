# How to Verify You're Connected to Ignix (Not Redis)

When using the Python and Node.js client examples, you might wonder: "How do I know I'm actually connecting to my Ignix server and not to some other Redis instance?"

Here are **5 definitive ways** to verify your connection:

## 🔍 Method 1: Process Verification

Check if the Ignix process is running:

```bash
ps aux | grep ignix
```

**Expected output:**
```
0xfd3495    41996   0.0  0.0  target/release/ignix
```

If you see `target/release/ignix`, that's **your Ignix server**!

## 🔍 Method 2: Port Verification

Check what's listening on port 7379:

```bash
lsof -i :7379
```

**Expected output:**
```
COMMAND   PID     USER   FD   TYPE     DEVICE SIZE/OFF NODE NAME
ignix   41996 0xfd3495    4u  IPv4 0x...      0t0  TCP *:7379 (LISTEN)
```

The **COMMAND** column shows `ignix` - that's your server!

## 🔍 Method 3: AOF File Verification

Ignix creates an `ignix.aof` file in the current directory:

```bash
ls -la ignix.aof
tail ignix.aof
```

**Expected output:**
```
-rw-r--r--  1 user  staff  714 Sep 22 18:06 ignix.aof
```

The file contains RESP commands that were executed. Every command you run gets logged here!

## 🔍 Method 4: Stop/Start Test

The most definitive test:

1. **Stop Ignix:**
   ```bash
   pkill -f ignix
   ```

2. **Try to connect with your client:**
   ```bash
   python3 examples/simple_python_client.py
   ```
   
   **Expected output:**
   ```
   ❌ Connection failed: [Errno 61] Connection refused
   ```

3. **Start Ignix again:**
   ```bash
   cargo run --release
   ```

4. **Try client again - it should work!**

If your client fails when Ignix is stopped and works when it's running, you're definitely connected to Ignix!

## 🔍 Method 5: Automated Verification Scripts

Run our verification scripts:

### Python Verification:
```bash
python3 examples/verify_connection.py
```

### Node.js Verification:
```bash
node examples/verify_connection.js
```

Both scripts will:
- ✅ Check if Ignix process is running
- ✅ Verify port 7379 is listening to Ignix
- ✅ Test AOF file creation/updates
- ✅ Execute test commands and verify responses
- ✅ Confirm data persistence

**Expected output:**
```
🎉 VERIFICATION SUCCESSFUL!
✅ You are connected to Ignix server
```

## 🚨 How to Spot if You're Connected to Redis Instead

If you accidentally connect to a Redis server instead of Ignix, you'll see:

1. **Process check:** `redis-server` instead of `ignix`
2. **Port check:** `redis-ser` instead of `ignix` in COMMAND column
3. **AOF file:** Either missing or in Redis format (different location/format)
4. **Commands:** Some Redis-specific commands might work that Ignix doesn't support

## 🎯 Quick Verification Checklist

- [ ] `ps aux | grep ignix` shows Ignix process
- [ ] `lsof -i :7379` shows `ignix` command
- [ ] `ignix.aof` file exists and gets updated
- [ ] Client fails when you stop Ignix (`pkill -f ignix`)
- [ ] Client works when you start Ignix (`cargo run --release`)

## 💡 Pro Tips

1. **Unique Test Data:** Use unique keys like `ignix_test_$(date +%s)` to verify your data is going to the right place

2. **Check AOF Contents:** 
   ```bash
   tail -f ignix.aof  # Watch commands in real-time
   ```

3. **Port Conflicts:** If you have Redis running on 6379 and Ignix on 7379, make sure your clients connect to **7379**

4. **Multiple Redis Instances:** If you have multiple Redis-like servers, check the process name in `ps aux` - only Ignix shows as `target/release/ignix`

## 🔧 Troubleshooting

**"I see redis-server in ps aux"**
- You're connected to Redis, not Ignix
- Make sure Ignix is running: `cargo run --release`
- Check your client connection port (should be 7379)

**"No ignix.aof file"**
- Ignix might not be running
- Check if you're in the right directory
- Run a few commands to trigger AOF writes

**"Connection refused"**
- Ignix is not running
- Start it: `cargo run --release`
- Check for port conflicts

---

**Bottom Line:** If you see `ignix` in your process list, `ignix.aof` getting updated, and your clients fail when you stop the Ignix process - you're definitely connected to Ignix! 🎉
