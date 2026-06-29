#!/bin/bash
# Key management for tunnel testing
# Usage: source test-scripts/keys.sh

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KEYS_DIR="$SCRIPT_DIR/.keys"
KEYS_FILE="$SCRIPT_DIR/.tunnel_keys"
TUNNEL_BIN="$SCRIPT_DIR/../target/release/tunnel-rs"

generate_keys() {
    echo "Generating new key pairs..."
    mkdir -p "$KEYS_DIR"

    # Generate server key
    "$TUNNEL_BIN" generate-server-key --output "$KEYS_DIR/server.key" --force 2>/dev/null
    SERVER_KEY_FILE="$KEYS_DIR/server.key"
    SERVER_NODE_ID=$("$TUNNEL_BIN" show-server-id --secret-file "$SERVER_KEY_FILE")

    # Generate auth token
    AUTH_TOKEN=$("$TUNNEL_BIN" generate-auth-token)

    # Save to config file
    cat > "$KEYS_FILE" << EOF
# Tunnel test keys - generated $(date)
SERVER_KEY_FILE=$SERVER_KEY_FILE
SERVER_NODE_ID=$SERVER_NODE_ID
TUNNEL_RS_AUTH_TOKEN=$AUTH_TOKEN
TUNNEL_RS_AUTH_TOKENS=$AUTH_TOKEN
EOF

    echo "Keys saved to $KEYS_DIR/"
    echo "  Server: $SERVER_NODE_ID"
}

load_keys() {
    if [ ! -f "$KEYS_FILE" ]; then
        echo "No keys file found. Generating new keys..."
        generate_keys
    fi
    source "$KEYS_FILE"
    export SERVER_KEY_FILE SERVER_NODE_ID TUNNEL_RS_AUTH_TOKEN TUNNEL_RS_AUTH_TOKENS
}

show_keys() {
    load_keys
    echo "=== Tunnel Test Keys ==="
    echo "Server Key:   $SERVER_KEY_FILE"
    echo "Server ID:    $SERVER_NODE_ID"
    echo "Auth Token:   $TUNNEL_RS_AUTH_TOKEN"
}

# Auto-load keys when sourced
load_keys
