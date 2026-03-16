# Multi-Session Testing Scripts

Scripts for testing client-initiated multi-session tunnel.

## Architecture

In client-initiated mode:
- **Server** whitelists allowed networks with `--allowed-tcp` (CIDR notation)
- **Client** specifies the source service with `--source` (`tcp://host:port` or `udp://host:port`)

```
[Echo Server]     [Server]                      [Client]                [Test Client]
  :19999    <--  --allowed-tcp 127.0.0.0/8  <--  --source tcp://localhost:19999  <-->  :17001-17003
                  (waits for connections)       (initiates, tests DNS)
```

## Quick Start

```bash
# Build first
cargo build --release

# Terminal 1: Start echo server (the source service)
python3 test-scripts/echo_server.py 19999

# Terminal 2: Start server (waits for client connections)
./test-scripts/server.sh

# Terminal 3: Start client(s) - these initiate connections to server
./test-scripts/client.sh 3      # 3 sessions on ports 17001-17003

# Terminal 4: Run tests
python3 test-scripts/test_tunnel.py -n 3                # Ping 3 ports (17001-17003)
python3 test-scripts/test_tunnel.py -n 3 --loop         # Ping every 5s
python3 test-scripts/test_tunnel.py -n 3 --stream 10    # Stream for 10s
python3 test-scripts/test_tunnel.py -n 3 --stream 10 --loop  # Stream 10s repeatedly
```

## Scripts

| Script | Description |
|--------|-------------|
| `server.sh [MAX]` | Start server with `--allowed-tcp 127.0.0.0/8` (default: max 5 sessions) |
| `client.sh [NUM] [PORT] [SRC]` | Start N clients requesting source SRC on local ports (default: 1 client, port 17001, source 19999) |
| `test_tunnel.py` | Test tunnel connectivity and data integrity |
| `echo_server.py [PORT]` | Multi-connection TCP echo server |
| `keys.sh` | Key management (auto-sourced by other scripts) |

## Key Management

Keys are auto-generated on first run:
- Server key saved to `test-scripts/.keys/server.key`
- Auth and ALPN tokens saved to `test-scripts/.tunnel_keys`

```bash
# View current keys
source test-scripts/keys.sh && show_keys

# Regenerate keys
source test-scripts/keys.sh && generate_keys

# Use keys in custom commands
source test-scripts/keys.sh
echo $SERVER_KEY_FILE $SERVER_NODE_ID $TUNNEL_RS_AUTH_TOKEN $TUNNEL_RS_ALPN_TOKEN
```

## Test Modes

### Ping Test
Send a single message and verify it echoes back:
```bash
python3 test-scripts/test_tunnel.py -n 3           # Once
python3 test-scripts/test_tunnel.py -n 3 --loop    # Every 5s
```

### Streaming Test
Concurrent streaming with data verification:
```bash
python3 test-scripts/test_tunnel.py -n 3 --stream 10           # 10 seconds
python3 test-scripts/test_tunnel.py -n 3 --stream 10 --loop    # 10s repeatedly
```

Output shows:
- Messages sent/received per session
- Bytes transferred
- Verified message counts
- Throughput stats

## Example Output

```
=== Streaming Test (10s) ===
Sessions: 3 (ports 17001-17003)
----------------------------------------------------------------------
[17001] Connected
[17002] Connected
[17003] Connected
[10.0s] Sent: 150.5KB, Recv: 180.2KB, Verified: 1500
----------------------------------------------------------------------
Results:
  [OK] Port 17001: sent=500 recv=495 verified=492 err=0
  [OK] Port 17002: sent=500 recv=492 verified=490 err=0
  [OK] Port 17003: sent=500 recv=493 verified=491 err=0
----------------------------------------------------------------------
Total: 150.5KB sent, 180.2KB recv, 1473 verified
Throughput: 15.0KB/s
*** ALL OK ***
```
