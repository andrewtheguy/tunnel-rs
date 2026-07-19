#!/usr/bin/env bash
#
# End-to-end test for tunnel-rs.
#
# Brings up, entirely on localhost:
#   * a Python TCP echo server and a Python UDP echo server (the "backend")
#   * a tunnel-rs server        (config fed as JSON on stdin, --config-stdin)
#   * a tunnel-rs TCP client    (config fed as JSON on stdin, --config-stdin)
#   * a tunnel-rs UDP client    (config fed as JSON on stdin, --config-stdin)
#
# then runs a Python echo client against each tunnel client's local port and
# verifies the payload makes the full round trip: client -> tunnel -> backend
# -> tunnel -> client.
#
# The Python backends/clients run via `uv run` (PEP 723 inline metadata, no
# third-party deps). The tunnel processes are the compiled Rust binary.
#
# Usage:
#   ./run_e2e.sh
#
# Environment overrides:
#   TUNNEL_RS_BIN   path to the tunnel-rs binary (default: cargo-built debug binary)
#   RELAY_URL       custom relay URL for both sides. When set, iroh discovery is
#                   disabled automatically and both sides rendezvous via the relay.
#                   When unset, the default public relay + iroh discovery server
#                   are used (requires internet access).
#   KEEP_LOGS       set to 1 to keep the temp working directory after the run.
#   READY_TIMEOUT   seconds to wait for each process to become ready (default: 60).
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
READY_TIMEOUT="${READY_TIMEOUT:-60}"

# ---------------------------------------------------------------------------
# Locate / build the binary
# ---------------------------------------------------------------------------
if [[ -n "${TUNNEL_RS_BIN:-}" ]]; then
    BIN="$TUNNEL_RS_BIN"
else
    BIN="$REPO_DIR/target/debug/tunnel-rs"
    if [[ ! -x "$BIN" ]]; then
        echo "==> Building tunnel-rs (debug)..."
        (cd "$REPO_DIR" && cargo build -q)
    fi
fi
if [[ ! -x "$BIN" ]]; then
    echo "ERROR: tunnel-rs binary not found at $BIN" >&2
    exit 1
fi

WORK="$(mktemp -d)"
declare -a PIDS=()

log() { printf '==> %s\n' "$*"; }

cleanup() {
    local status=$?
    for pid in "${PIDS[@]:-}"; do
        [[ -n "$pid" ]] || continue
        # Each process is a session leader (setsid), so kill the whole group.
        kill -TERM -- "-$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    if [[ "${KEEP_LOGS:-0}" == "1" ]]; then
        echo "==> Logs kept in $WORK"
    else
        rm -rf "$WORK"
    fi
    exit "$status"
}
trap cleanup EXIT INT TERM

# Start a background process in its own session (so cleanup can kill the whole
# group). Args: <logfile> <command...>. Records the PID in PIDS.
start_bg() {
    local logfile="$1"; shift
    setsid "$@" >"$logfile" 2>&1 &
    local pid=$!
    PIDS+=("$pid")
    echo "$pid"
}

# Wait until $1 (a log file) contains the regex $2, or time out after $3 secs.
wait_for_log() {
    local logfile="$1" pattern="$2" timeout="$3"
    local max_attempts=$(( timeout * 2 )) attempt=0
    while (( attempt < max_attempts )); do
        if [[ -f "$logfile" ]] && grep -Eq "$pattern" "$logfile"; then
            return 0
        fi
        sleep 0.5
        attempt=$(( attempt + 1 ))
    done
    echo "ERROR: timed out after ${timeout}s waiting for /$pattern/ in $logfile" >&2
    echo "----- $logfile -----" >&2
    cat "$logfile" >&2 || true
    return 1
}

# ---------------------------------------------------------------------------
# Allocate free localhost ports (kept bound until the picker exits to minimise
# collisions between the four).
# ---------------------------------------------------------------------------
read -r TCP_BACKEND UDP_BACKEND TCP_TARGET UDP_TARGET < <(python3 - <<'PY'
import socket
socks = []
for _ in range(4):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.bind(("127.0.0.1", 0))
    socks.append(s)
print(" ".join(str(s.getsockname()[1]) for s in socks))
PY
)
log "Ports: tcp_backend=$TCP_BACKEND udp_backend=$UDP_BACKEND tcp_target=$TCP_TARGET udp_target=$UDP_TARGET"

# ---------------------------------------------------------------------------
# Server identity + auth token
# ---------------------------------------------------------------------------
SECRET="$("$BIN" generate-server-key --output - 2>"$WORK/keygen.err")"
ENDPOINT_ID="$(sed -n 's/^EndpointId: //p' "$WORK/keygen.err")"
if [[ -z "$ENDPOINT_ID" ]]; then
    echo "ERROR: could not parse EndpointId from keygen" >&2
    cat "$WORK/keygen.err" >&2
    exit 1
fi
TOKEN="$("$BIN" generate-auth-token)"
log "EndpointId: $ENDPOINT_ID"

# Optional custom-relay JSON fragment (exercises the discovery-disabled path).
RELAY_FRAGMENT=""
if [[ -n "${RELAY_URL:-}" ]]; then
    RELAY_FRAGMENT=",\"relay_urls\":[\"$RELAY_URL\"]"
    log "Using custom relay: $RELAY_URL (iroh discovery disabled)"
else
    log "Using default relay + iroh discovery server (needs internet)"
fi

# ---------------------------------------------------------------------------
# Start Python echo backends (via uv) and wait for them to bind
# ---------------------------------------------------------------------------
log "Starting Python echo backends via uv..."
start_bg "$WORK/echo_tcp.log" uv run "$SCRIPT_DIR/echo_server.py" \
    --proto tcp --host 127.0.0.1 --port "$TCP_BACKEND" >/dev/null
start_bg "$WORK/echo_udp.log" uv run "$SCRIPT_DIR/echo_server.py" \
    --proto udp --host 127.0.0.1 --port "$UDP_BACKEND" >/dev/null
wait_for_log "$WORK/echo_tcp.log" "READY tcp" 30
wait_for_log "$WORK/echo_udp.log" "READY udp" 30

# ---------------------------------------------------------------------------
# tunnel-rs server (JSON config on stdin)
# ---------------------------------------------------------------------------
cat >"$WORK/server.json" <<EOF
{
  "role": "server",
  "mode": "iroh",
  "iroh": {
    "secret": "$SECRET",
    "auth_tokens": ["$TOKEN"],
    "allowed_sources": { "tcp": ["127.0.0.0/8"], "udp": ["127.0.0.0/8"] }$RELAY_FRAGMENT
  }
}
EOF

log "Starting tunnel-rs server..."
setsid "$BIN" server --config-stdin <"$WORK/server.json" >"$WORK/server.log" 2>&1 &
PIDS+=("$!")
wait_for_log "$WORK/server.log" "Waiting for clients to connect" "$READY_TIMEOUT"

# ---------------------------------------------------------------------------
# tunnel-rs clients (one per protocol; JSON config on stdin)
# ---------------------------------------------------------------------------
cat >"$WORK/client_tcp.json" <<EOF
{
  "role": "client",
  "mode": "iroh",
  "iroh": {
    "server_node_id": "$ENDPOINT_ID",
    "request_source": "tcp://127.0.0.1:$TCP_BACKEND",
    "target": "127.0.0.1:$TCP_TARGET",
    "auth_token": "$TOKEN"$RELAY_FRAGMENT
  }
}
EOF

cat >"$WORK/client_udp.json" <<EOF
{
  "role": "client",
  "mode": "iroh",
  "iroh": {
    "server_node_id": "$ENDPOINT_ID",
    "request_source": "udp://127.0.0.1:$UDP_BACKEND",
    "target": "127.0.0.1:$UDP_TARGET",
    "auth_token": "$TOKEN"$RELAY_FRAGMENT
  }
}
EOF

log "Starting tunnel-rs TCP client..."
setsid "$BIN" client --config-stdin <"$WORK/client_tcp.json" >"$WORK/client_tcp.log" 2>&1 &
PIDS+=("$!")
log "Starting tunnel-rs UDP client..."
setsid "$BIN" client --config-stdin <"$WORK/client_udp.json" >"$WORK/client_udp.log" 2>&1 &
PIDS+=("$!")

wait_for_log "$WORK/client_tcp.log" "Listening on TCP" "$READY_TIMEOUT"
wait_for_log "$WORK/client_udp.log" "Listening on UDP" "$READY_TIMEOUT"

# ---------------------------------------------------------------------------
# Run the echo test clients through the tunnel
# ---------------------------------------------------------------------------
RESULT=0

log "Testing TCP round trip through tunnel (127.0.0.1:$TCP_TARGET)..."
if uv run "$SCRIPT_DIR/echo_client.py" --proto tcp --host 127.0.0.1 \
        --port "$TCP_TARGET" --message "hello-tcp-$(date +%s)"; then
    log "TCP round trip: PASS"
else
    log "TCP round trip: FAIL"
    RESULT=1
fi

log "Testing UDP round trip through tunnel (127.0.0.1:$UDP_TARGET)..."
if uv run "$SCRIPT_DIR/echo_client.py" --proto udp --host 127.0.0.1 \
        --port "$UDP_TARGET" --message "hello-udp-$(date +%s)"; then
    log "UDP round trip: PASS"
else
    log "UDP round trip: FAIL"
    RESULT=1
fi

echo
if [[ "$RESULT" -eq 0 ]]; then
    log "E2E RESULT: ALL PASS ✅"
else
    log "E2E RESULT: FAILURES ❌ (re-run with KEEP_LOGS=1 to inspect $WORK)"
fi
exit "$RESULT"
