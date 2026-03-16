#!/bin/bash
# Start tunnel client(s)
# Usage: ./test-scripts/client.sh [NUM_SESSIONS] [BASE_PORT] [SOURCE_PORT]
#
# The client specifies the source service to tunnel (--source) and
# exposes it on local target ports (--target).

set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/keys.sh"

NUM_SESSIONS="${1:-1}"
BASE_PORT="${2:-17001}"
SOURCE_PORT="${3:-19999}"
TUNNEL_BIN="$SCRIPT_DIR/../target/release/tunnel-rs"

[ ! -f "$TUNNEL_BIN" ] && cargo build --release --manifest-path="$SCRIPT_DIR/../Cargo.toml"

PIDS=()
cleanup() {
    echo "Stopping all clients..."
    for pid in "${PIDS[@]}"; do kill $pid 2>/dev/null || true; done
}
trap cleanup EXIT

echo "=== Client ==="
echo "Source: tcp://localhost:$SOURCE_PORT (on server's side)"
echo "Sessions: $NUM_SESSIONS (local ports $BASE_PORT-$((BASE_PORT + NUM_SESSIONS - 1)))"
echo ""

for i in $(seq 1 $NUM_SESSIONS); do
    PORT=$((BASE_PORT + i - 1))
    echo "[$i] Starting client on port $PORT..."
    "$TUNNEL_BIN" client \
        --source "tcp://localhost:$SOURCE_PORT" \
        --target "127.0.0.1:$PORT" \
        --server-node-id "$SERVER_NODE_ID" \
        --auth-token "$AUTH_TOKEN" \
        --alpn-token "$ALPN_TOKEN" &
    PIDS+=($!)
    [ $NUM_SESSIONS -gt 1 ] && sleep 2  # Stagger for rate limits
done

echo ""
echo "=== Ready ==="
echo "Test with: python3 test-scripts/test_tunnel.py -n $NUM_SESSIONS --port $BASE_PORT --loop"
echo "Press Ctrl+C to stop"
wait
