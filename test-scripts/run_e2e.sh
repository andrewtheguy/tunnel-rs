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
#   RELAY_URL       custom relay URL for both sides. A single custom relay uses
#                   discovery="none"; multiple relays retain public discovery
#                   because lookup is needed to identify the server's home relay.
#                   When unset, the default public relay + discovery server are
#                   used (requires internet access).
#   KEEP_LOGS       set to 1 to keep the temp working directory after the run.
#   READY_TIMEOUT   seconds to wait for each process to become ready (default: 60).
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
READY_TIMEOUT="${READY_TIMEOUT:-60}"

# ---------------------------------------------------------------------------
# CLI arguments
# ---------------------------------------------------------------------------
declare -a RELAY_URLS=()
RELAY_ONLY=0

usage() {
    cat <<'USAGE'
Usage: run_e2e.sh [OPTIONS]

Run the tunnel-rs TCP/UDP end-to-end test on localhost.

With no options it runs the default test: the public iroh relay plus the
default iroh discovery server (requires internet access), no relay override.

Options:
  --relay-url URL   Custom relay URL for both sides (repeatable). One relay uses
                    discovery="none"; multiple relays keep public discovery.
                    May also be given as --relay-url=URL.
  --relay-only      Force all traffic through the relay, disabling direct P2P.
                    Requires at least one --relay-url.
  -h, --help        Show this help and exit.

Environment overrides: TUNNEL_RS_BIN, READY_TIMEOUT, KEEP_LOGS, RELAY_URL
(RELAY_URL is a fallback used only when no --relay-url flag is given).
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --relay-url)
            shift
            [[ $# -gt 0 ]] || { echo "ERROR: --relay-url requires a value" >&2; exit 2; }
            RELAY_URLS+=("$1")
            ;;
        --relay-url=*)
            RELAY_URLS+=("${1#*=}")
            ;;
        --relay-only)
            RELAY_ONLY=1
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "ERROR: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
    shift
done

# Back-compat: honor the RELAY_URL env var only when no --relay-url flag given.
if [[ ${#RELAY_URLS[@]} -eq 0 && -n "${RELAY_URL:-}" ]]; then
    RELAY_URLS+=("$RELAY_URL")
fi

if [[ "$RELAY_ONLY" == "1" && ${#RELAY_URLS[@]} -eq 0 ]]; then
    echo "ERROR: --relay-only requires at least one --relay-url" >&2
    exit 2
fi

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

# Start tunnel-rs with an in-memory JSON config fed through a pipe. Secrets are
# never written to the temporary working directory.
start_tunnel() {
    local role="$1" config="$2" logfile="$3"
    printf '%s\n' "$config" |
        setsid "$BIN" "$role" --config-stdin "${RELAY_ONLY_ARGS[@]}" >"$logfile" 2>&1 &
    PIDS+=("$!")
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
# One socket per port, matching the protocol each port will actually serve, so
# the ephemeral port is guaranteed free in the right (TCP vs UDP) namespace.
# Order: TCP_BACKEND, UDP_BACKEND, TCP_TARGET, UDP_TARGET.
kinds = [socket.SOCK_STREAM, socket.SOCK_DGRAM, socket.SOCK_STREAM, socket.SOCK_DGRAM]
socks = []
for kind in kinds:
    s = socket.socket(socket.AF_INET, kind)
    s.bind(("127.0.0.1", 0))
    socks.append(s)
print(" ".join(str(s.getsockname()[1]) for s in socks))
PY
)
log "Ports: tcp_backend=$TCP_BACKEND udp_backend=$UDP_BACKEND tcp_target=$TCP_TARGET udp_target=$UDP_TARGET"

# ---------------------------------------------------------------------------
# Server identity + auth token
# ---------------------------------------------------------------------------
read -r ENDPOINT_ID SECRET < <(
    "$BIN" generate-server-key --json |
        python3 -c 'import json, sys; value = json.load(sys.stdin); print(value["public_key"], value["private_key"])'
)
TOKEN="$(
    "$BIN" generate-auth-token --json |
        python3 -c 'import json, sys; print(json.load(sys.stdin)["auth_tokens"][0])'
)"
log "EndpointId: $ENDPOINT_ID"

# Optional custom-relay configuration and matching --relay-only CLI args
# (relay_only is CLI-only, not config).
RELAY_CONFIG="{}"
declare -a RELAY_ONLY_ARGS=()
if [[ ${#RELAY_URLS[@]} -gt 0 ]]; then
    RELAY_CONFIG="$(
        printf '%s\0' "${RELAY_URLS[@]}" |
            python3 -c 'import json, sys; urls = [value.decode() for value in sys.stdin.buffer.read().split(b"\0") if value]; print(json.dumps({"relay_urls": urls}))'
    )"
    if [[ ${#RELAY_URLS[@]} -eq 1 ]]; then
        RELAY_CONFIG="$(
            printf '%s\n' "$RELAY_CONFIG" |
                python3 -c 'import json, sys; value = json.load(sys.stdin); value["discovery"] = "none"; print(json.dumps(value))'
        )"
        log "Using one custom relay with discovery=none: ${RELAY_URLS[*]}"
    else
        log "Using custom relays with public discovery: ${RELAY_URLS[*]}"
    fi
else
    log "Using default relay + iroh discovery server (needs internet)"
fi
if [[ "$RELAY_ONLY" == "1" ]]; then
    RELAY_ONLY_ARGS+=(--relay-only)
    log "Relay-only mode: direct P2P disabled, all traffic via relay"
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
# tunnel-rs server (in-memory JSON config piped to stdin)
# ---------------------------------------------------------------------------
SERVER_CONFIG="$(
    printf '%s\n%s\n%s\n' "$SECRET" "$TOKEN" "$RELAY_CONFIG" |
        python3 -c '
import json, sys
secret = sys.stdin.readline().rstrip("\n")
token = sys.stdin.readline().rstrip("\n")
iroh = {
    "secret": secret,
    "auth_tokens": [token],
    "allowed_sources": {"tcp": ["127.0.0.0/8"], "udp": ["127.0.0.0/8"]},
}
iroh.update(json.loads(sys.stdin.readline()))
print(json.dumps({"role": "server", "mode": "iroh", "iroh": iroh}))
'
)"

log "Starting tunnel-rs server..."
start_tunnel server "$SERVER_CONFIG" "$WORK/server.log"
unset SERVER_CONFIG
wait_for_log "$WORK/server.log" "Waiting for clients to connect" "$READY_TIMEOUT"

# ---------------------------------------------------------------------------
# tunnel-rs clients (one per protocol; in-memory JSON configs piped to stdin)
# ---------------------------------------------------------------------------
build_client_config() {
    local protocol="$1" backend_port="$2" target_port="$3"
    printf '%s\n%s\n%s\n' "$ENDPOINT_ID" "$TOKEN" "$RELAY_CONFIG" |
        python3 -c '
import json, sys
protocol, backend_port, target_port = sys.argv[1:]
endpoint_id = sys.stdin.readline().rstrip("\n")
token = sys.stdin.readline().rstrip("\n")
iroh = {
    "server_node_id": endpoint_id,
    "request_source": f"{protocol}://127.0.0.1:{backend_port}",
    "target": f"127.0.0.1:{target_port}",
    "auth_token": token,
}
iroh.update(json.loads(sys.stdin.readline()))
print(json.dumps({"role": "client", "mode": "iroh", "iroh": iroh}))
' "$protocol" "$backend_port" "$target_port"
}

CLIENT_TCP_CONFIG="$(build_client_config tcp "$TCP_BACKEND" "$TCP_TARGET")"
CLIENT_UDP_CONFIG="$(build_client_config udp "$UDP_BACKEND" "$UDP_TARGET")"

log "Starting tunnel-rs TCP client..."
start_tunnel client "$CLIENT_TCP_CONFIG" "$WORK/client_tcp.log"
log "Starting tunnel-rs UDP client..."
start_tunnel client "$CLIENT_UDP_CONFIG" "$WORK/client_udp.log"
unset CLIENT_TCP_CONFIG CLIENT_UDP_CONFIG RELAY_CONFIG SECRET TOKEN

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
