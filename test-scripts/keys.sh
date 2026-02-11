#!/bin/bash
# Key management for tunnel testing
# Usage: source test-scripts/keys.sh

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KEYS_DIR="$SCRIPT_DIR/.keys"
KEYS_FILE="$SCRIPT_DIR/.tunnel_keys"
TUNNEL_BIN="$SCRIPT_DIR/../target/release/tunnel-rs-ice"

generate_keys() {
    echo "Generating new key pairs..."
    mkdir -p "$KEYS_DIR"

    # Generate server keys (nsec saved to file, npub printed to stdout)
    SERVER_NPUB=$("$TUNNEL_BIN" generate-nostr-key --output "$KEYS_DIR/server.nsec" --force 2>/dev/null | grep "^npub:" | awk '{print $2}')
    SERVER_NSEC_FILE="$KEYS_DIR/server.nsec"

    # Generate client keys
    CLIENT_NPUB=$("$TUNNEL_BIN" generate-nostr-key --output "$KEYS_DIR/client.nsec" --force 2>/dev/null | grep "^npub:" | awk '{print $2}')
    CLIENT_NSEC_FILE="$KEYS_DIR/client.nsec"

    # Save paths and npubs to config file
    cat > "$KEYS_FILE" << EOF
# Tunnel test keys - generated $(date)
SERVER_NSEC_FILE=$SERVER_NSEC_FILE
SERVER_NPUB=$SERVER_NPUB
CLIENT_NSEC_FILE=$CLIENT_NSEC_FILE
CLIENT_NPUB=$CLIENT_NPUB
EOF

    echo "Keys saved to $KEYS_DIR/"
    echo "  Server: $SERVER_NPUB"
    echo "  Client: $CLIENT_NPUB"
}

load_keys() {
    if [ ! -f "$KEYS_FILE" ]; then
        echo "No keys file found. Generating new keys..."
        generate_keys
    fi
    source "$KEYS_FILE"
    export SERVER_NSEC_FILE SERVER_NPUB CLIENT_NSEC_FILE CLIENT_NPUB
}

show_keys() {
    load_keys
    echo "=== Tunnel Test Keys ==="
    echo "Server NSEC:  $SERVER_NSEC_FILE"
    echo "Server NPUB:  $SERVER_NPUB"
    echo "Client NSEC:  $CLIENT_NSEC_FILE"
    echo "Client NPUB:  $CLIENT_NPUB"
}

# Auto-load keys when sourced
load_keys
