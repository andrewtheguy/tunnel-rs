#!/usr/bin/env bash
#
# Relay failover end-to-end test for tunnel-rs (relay-only mode, no internet).
#
# Runs TWO local iroh-relay instances (`--dev` mode, plain HTTP) and exercises
# relay failure scenarios. Servers and clients are each configured with an
# explicit relay list per scenario. Custom relays disable internet discovery
# automatically, so the whole test runs without any public iroh infrastructure.
#
# Startup contract under test: every *configured* custom relay is probed
# individually at startup and ALL of them must come online, so a dead relay in
# the configured set is fatal even when another relay would work. Failover is a
# RUNTIME property: once a process has started, losing a relay is survivable and
# the peer re-homes onto a surviving one.
#
# Phase A - relay offline BEFORE startup (the per-relay startup probe):
#   A0  both relays down; server configured with BOTH ..... startup fails (negative)
#   A1  only relay2 up; server configured with BOTH ....... startup fails (negative)
#   A2  only relay2 up; server AND client configured with
#       ONLY relay2 ......................................  connects; TCP echo passes
#   A3  same server; client configured with BOTH .......... startup fails (negative)
#   A4  same server; client configured with ONLY the dead
#       relay1 ...........................................  startup fails (negative)
#
# Phase B - relay offline AFTER startup (runtime failover):
#   B1  both relays up; server and client with both relays; connects; TCP echo passes
#   B2  the relay carrying the connection is killed; the server stays up and a
#       restarted client configured with ONLY the survivor reconnects once the
#       server re-homes (iroh re-probes relays every ~20-26s); TCP echo passes
#   B3  the surviving relay is killed too (both down); a new client fails (negative)
#   B4  both relays are restarted; a new client with both relays connects again
#       (server relay reconnect + re-home); TCP echo passes
#
# Requirements: iroh-relay (cargo install iroh-relay), uv, python3.
#
# Usage:
#   ./run_relay_failover_e2e.sh
#
# Environment overrides:
#   TUNNEL_RS_BIN   path to the tunnel-rs binary (default: cargo-built debug binary)
#   IROH_RELAY_BIN  path to the iroh-relay binary (default: iroh-relay on PATH)
#   KEEP_LOGS       set to 1 to keep the working directory after the run.
#   READY_TIMEOUT   seconds to wait for each process to become ready (default: 60).
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
READY_TIMEOUT="${READY_TIMEOUT:-60}"

# ---------------------------------------------------------------------------
# Locate binaries
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
[[ -x "$BIN" ]] || { echo "ERROR: tunnel-rs binary not found at $BIN" >&2; exit 1; }

RELAY_BIN="${IROH_RELAY_BIN:-$(command -v iroh-relay || true)}"
[[ -n "$RELAY_BIN" && -x "$RELAY_BIN" ]] || {
    echo "ERROR: iroh-relay not found. Install with: cargo install iroh-relay" >&2
    exit 1
}

# ---------------------------------------------------------------------------
# Working directory (repo ./tmp) + process management
# ---------------------------------------------------------------------------
mkdir -p "$REPO_DIR/tmp"
WORK="$(mktemp -d "$REPO_DIR/tmp/relay-failover.XXXXXX")"
declare -a PIDS=()

log()  { printf '==> %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }

cleanup() {
    local status=$?
    for pid in "${PIDS[@]:-}"; do
        [[ -n "$pid" ]] || continue
        kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
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

kill_pid() {
    local pid="$1"
    [[ -n "$pid" ]] || return 0
    kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
    # Reap so the port is really free before a restart.
    for _ in $(seq 1 50); do
        kill -0 "$pid" 2>/dev/null || return 0
        sleep 0.1
    done
    kill -KILL -- "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null || true
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
    return 1
}

# Like wait_for_log, but gives up early (rc 2) if process $1 exits first.
wait_for_log_or_death() {
    local pid="$1" logfile="$2" pattern="$3" timeout="$4"
    local max_attempts=$(( timeout * 2 )) attempt=0
    while (( attempt < max_attempts )); do
        if [[ -f "$logfile" ]] && grep -Eq "$pattern" "$logfile"; then
            return 0
        fi
        if ! kill -0 "$pid" 2>/dev/null; then
            # One last look: the pattern may have landed just before exit.
            grep -Eq "$pattern" "$logfile" 2>/dev/null && return 0
            return 2
        fi
        sleep 0.5
        attempt=$(( attempt + 1 ))
    done
    return 1
}

wait_for_tcp_port() {
    local port="$1" timeout="$2"
    python3 - "$port" "$timeout" <<'PY'
import socket, sys, time
port, timeout = int(sys.argv[1]), float(sys.argv[2])
deadline = time.monotonic() + timeout
while time.monotonic() < deadline:
    try:
        socket.create_connection(("127.0.0.1", port), timeout=1).close()
        sys.exit(0)
    except OSError:
        time.sleep(0.25)
sys.exit(1)
PY
}

pick_port() {
    python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

# ---------------------------------------------------------------------------
# Ports, identity, client authentication key
# ---------------------------------------------------------------------------
RELAY1_PORT="$(pick_port)"
RELAY2_PORT="$(pick_port)"
BACKEND_PORT="$(pick_port)"
RELAY1_URL="http://127.0.0.1:$RELAY1_PORT"
RELAY2_URL="http://127.0.0.1:$RELAY2_PORT"
log "Relays: relay1=$RELAY1_URL relay2=$RELAY2_URL backend=127.0.0.1:$BACKEND_PORT"

read -r ENDPOINT_ID SECRET < <(
    "$BIN" generate-server-key --json |
        python3 -c 'import json, sys; value = json.load(sys.stdin); print(value["public_key"], value["private_key"])'
)
"$BIN" generate-auth-key --output "$WORK/client.key" --comment "failover e2e client" \
    > "$WORK/authorized_keys"
log "EndpointId: $ENDPOINT_ID"

# ---------------------------------------------------------------------------
# Relay management (config files contain no secrets)
# ---------------------------------------------------------------------------
RELAY1_PID=""
RELAY2_PID=""

write_relay_config() {
    local num="$1" port="$2"
    cat > "$WORK/relay$num.toml" <<EOF
enable_metrics = false
http_bind_addr = "127.0.0.1:$port"
EOF
}
write_relay_config 1 "$RELAY1_PORT"
write_relay_config 2 "$RELAY2_PORT"

start_relay() {
    local num="$1" port logfile
    port="$(eval echo "\$RELAY${num}_PORT")"
    logfile="$WORK/relay$num.$(date +%s%N).log"
    setsid "$RELAY_BIN" --dev -c "$WORK/relay$num.toml" >"$logfile" 2>&1 &
    local pid=$!
    PIDS+=("$pid")
    eval "RELAY${num}_PID=$pid"
    wait_for_tcp_port "$port" 30 || {
        echo "ERROR: relay$num did not start; log:" >&2
        cat "$logfile" >&2
        return 1
    }
    note "relay$num up (pid $pid, port $port)"
}

stop_relay() {
    local num="$1" pid
    pid="$(eval echo "\$RELAY${num}_PID")"
    kill_pid "$pid"
    eval "RELAY${num}_PID="
    note "relay$num stopped"
}

# ---------------------------------------------------------------------------
# tunnel-rs server / client management (JSON configs piped to stdin)
# ---------------------------------------------------------------------------
SERVER_PID=""
SERVER_LOG=""

# Start the tunnel server in relay-only mode. Args: <relay_url>...
start_server() {
    SERVER_LOG="$WORK/server.$(date +%s%N).log"
    local config
    config="$(
        printf '%s\n' "$SECRET" |
            python3 -c '
import json, sys
secret = sys.stdin.readline().rstrip("\n")
iroh = {
    "secret": secret,
    "authorized_keys_file": sys.argv[1],
    "allowed_sources": {"tcp": ["127.0.0.0/8"]},
    "relay_urls": sys.argv[2:],
}
print(json.dumps({"role": "server", "mode": "iroh", "iroh": iroh}))
' "$WORK/authorized_keys" "$@"
    )"
    printf '%s\n' "$config" |
        setsid "$BIN" server --config-stdin --relay-only >"$SERVER_LOG" 2>&1 &
    SERVER_PID=$!
    PIDS+=("$SERVER_PID")
}

# Start a server that is EXPECTED to fail its startup relay probe. Passes when
# the process reports the failure and never becomes ready. Args: <relay_url>...
expect_server_start_failure() {
    local rc=0
    start_server "$@"
    wait_for_log_or_death "$SERVER_PID" "$SERVER_LOG" \
        "failed to come online" "$READY_TIMEOUT" || rc=$?
    # rc 0 (error logged) or 2 (process died after logging) both mean it gave
    # up; verify it never became ready.
    if grep -Eq "Waiting for clients to connect" "$SERVER_LOG"; then
        rc=1
    elif [[ "$rc" -eq 2 ]]; then
        rc=0
    fi
    stop_server
    return "$rc"
}

# Start a server that is EXPECTED to come up. Args: <relay_url>...
expect_server_ready() {
    local rc=0
    start_server "$@"
    wait_for_log_or_death "$SERVER_PID" "$SERVER_LOG" \
        "Waiting for clients to connect" "$READY_TIMEOUT" || rc=1
    return "$rc"
}

stop_server() {
    kill_pid "$SERVER_PID"
    SERVER_PID=""
}

# Start a tunnel client. Args: <target_port> <logfile> <relay_url>...
start_client() {
    local target_port="$1" logfile="$2"; shift 2
    local config
    config="$(
        printf '%s\n' "$ENDPOINT_ID" |
            python3 -c '
import json, sys
target_port = sys.argv[1]
backend_port = sys.argv[2]
endpoint_id = sys.stdin.readline().rstrip("\n")
iroh = {
    "server_node_id": endpoint_id,
    "request_source": f"tcp://127.0.0.1:{backend_port}",
    "target": f"127.0.0.1:{target_port}",
    "private_key_file": sys.argv[3],
    "relay_urls": sys.argv[4:],
}
print(json.dumps({"role": "client", "mode": "iroh", "iroh": iroh}))
' "$target_port" "$BACKEND_PORT" "$WORK/client.key" "$@"
    )"
    printf '%s\n' "$config" |
        setsid "$BIN" client --config-stdin --relay-only >"$logfile" 2>&1 &
    CLIENT_PID=$!
    PIDS+=("$CLIENT_PID")
}

# Run one TCP echo round trip through target port $1.
echo_check() {
    local target_port="$1"
    uv run "$SCRIPT_DIR/echo_client.py" --proto tcp --host 127.0.0.1 \
        --port "$target_port" --message "failover-$(date +%s%N)"
}

CLIENT_PID=""
CLIENT_LOG=""
CLIENT_TARGET_PORT=""

# Start a client (relay list in "$@"), wait for the tunnel to establish, and
# push one echo through it. Retries with a fresh client process, because after
# a relay failure the server needs a re-probe cycle (~20-26s) to re-home.
# On success leaves the client running (CLIENT_PID/CLIENT_LOG/CLIENT_TARGET_PORT).
connect_and_echo() {
    local attempts="$1"; shift
    local attempt rc
    for (( attempt = 1; attempt <= attempts; attempt++ )); do
        CLIENT_TARGET_PORT="$(pick_port)"
        CLIENT_LOG="$WORK/client.$(date +%s%N).log"
        start_client "$CLIENT_TARGET_PORT" "$CLIENT_LOG" "$@"
        rc=0
        wait_for_log_or_death "$CLIENT_PID" "$CLIENT_LOG" "Listening on TCP" "$READY_TIMEOUT" || rc=$?
        if [[ "$rc" -eq 0 ]] && echo_check "$CLIENT_TARGET_PORT"; then
            return 0
        fi
        note "attempt $attempt/$attempts failed (rc=$rc), retrying..."
        kill_pid "$CLIENT_PID"
        CLIENT_PID=""
        sleep 3
    done
    echo "----- last client log -----" >&2
    cat "$CLIENT_LOG" >&2 || true
    return 1
}

# Start a client that is EXPECTED to fail to connect. Passes when the process
# exits without ever establishing the tunnel.
expect_connect_failure() {
    local logfile="$WORK/client.$(date +%s%N).log"
    local target_port rc=0
    target_port="$(pick_port)"
    start_client "$target_port" "$logfile" "$@"
    wait_for_log_or_death "$CLIENT_PID" "$logfile" "Listening on TCP" "$READY_TIMEOUT" || rc=$?
    if [[ "$rc" -eq 0 ]]; then
        note "unexpectedly connected"
        kill_pid "$CLIENT_PID"; CLIENT_PID=""
        return 1
    fi
    kill_pid "$CLIENT_PID"; CLIENT_PID=""
    if [[ "$rc" -eq 2 ]]; then
        return 0   # process died before establishing the tunnel, as expected
    fi
    note "client neither connected nor exited within ${READY_TIMEOUT}s"
    return 1
}

# ---------------------------------------------------------------------------
# Scenario bookkeeping
# ---------------------------------------------------------------------------
RESULT=0
declare -a SUMMARY=()

scenario() {
    local id="$1" desc="$2"
    log "[$id] $desc"
}

record() {
    local id="$1" ok="$2"
    if [[ "$ok" -eq 0 ]]; then
        SUMMARY+=("PASS  $id")
        log "[$id] PASS"
    else
        SUMMARY+=("FAIL  $id")
        log "[$id] FAIL"
        RESULT=1
    fi
}

# ---------------------------------------------------------------------------
# Echo backend (shared by all scenarios)
# ---------------------------------------------------------------------------
log "Starting TCP echo backend via uv..."
setsid uv run "$SCRIPT_DIR/echo_server.py" --proto tcp --host 127.0.0.1 \
    --port "$BACKEND_PORT" >"$WORK/echo_tcp.log" 2>&1 &
PIDS+=("$!")
wait_for_log "$WORK/echo_tcp.log" "READY tcp" 30

# ===========================================================================
# Phase A - relays offline BEFORE startup (the per-relay startup probe)
# ===========================================================================

scenario A0 "both relays down: server configured with both fails to start"
rc=0
expect_server_start_failure "$RELAY1_URL" "$RELAY2_URL" || rc=1
record A0 "$rc"

scenario A1 "relay1 down: server configured with BOTH relays still fails to start"
start_relay 2
rc=0
expect_server_start_failure "$RELAY1_URL" "$RELAY2_URL" || rc=1
record A1 "$rc"

scenario A2 "server and client configured with ONLY the live relay (relay2) connect"
rc=0
expect_server_ready "$RELAY2_URL" || rc=1
if [[ "$rc" -eq 0 ]]; then
    connect_and_echo 3 "$RELAY2_URL" || rc=1
fi
record A2 "$rc"
kill_pid "$CLIENT_PID"; CLIENT_PID=""

scenario A3 "client configured with BOTH relays fails (relay1 is down)"
rc=0
expect_connect_failure "$RELAY1_URL" "$RELAY2_URL" || rc=1
record A3 "$rc"

scenario A4 "client configured with ONLY the dead relay (relay1) fails"
rc=0
expect_connect_failure "$RELAY1_URL" || rc=1
record A4 "$rc"

stop_server
stop_relay 2

# ===========================================================================
# Phase B - relays go offline AFTER client/server are connected
# ===========================================================================

scenario B1 "both relays up: client with both relays connects"
start_relay 1
start_relay 2
rc=0
expect_server_ready "$RELAY1_URL" "$RELAY2_URL" || rc=1
CONNECTED_RELAY_NUM=""
if [[ "$rc" -eq 0 ]]; then
    connect_and_echo 3 "$RELAY1_URL" "$RELAY2_URL" || rc=1
fi
if [[ "$rc" -eq 0 ]]; then
    connected_url="$(grep -Eo "Connected via relay: [^ ]+" "$CLIENT_LOG" | tail -1 | awk '{print $NF}')"
    case "$connected_url" in
        *":$RELAY1_PORT"*) CONNECTED_RELAY_NUM=1 ;;
        *":$RELAY2_PORT"*) CONNECTED_RELAY_NUM=2 ;;
        *) note "could not determine connected relay from: $connected_url"; rc=1 ;;
    esac
    note "connection is via relay$CONNECTED_RELAY_NUM ($connected_url)"
fi
record B1 "$rc"

scenario B2 "kill the relay carrying the connection: server survives, client reconnects via the survivor"
rc=0
if [[ -n "$CONNECTED_RELAY_NUM" ]]; then
    SURVIVOR_NUM=$(( 3 - CONNECTED_RELAY_NUM ))
    SURVIVOR_URL="$(eval echo "\$RELAY${SURVIVOR_NUM}_URL")"
    stop_relay "$CONNECTED_RELAY_NUM"
    # The old client's QUIC connection lingers until it times out; restart the
    # client instead (real deployments restart via a supervisor).
    kill_pid "$CLIENT_PID"; CLIENT_PID=""
    # Losing a relay at runtime must NOT take the already-started server down;
    # only the startup probe is strict.
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        note "server exited when a relay died at runtime"
        rc=1
    fi
    # The restarted client is configured with only the surviving relay: the
    # startup probe would reject the dead one. The server needs a net_report
    # re-probe cycle (~20-26s) to re-home onto it; retry.
    if [[ "$rc" -eq 0 ]]; then
        connect_and_echo 6 "$SURVIVOR_URL" || rc=1
    fi
else
    rc=1
fi
record B2 "$rc"
kill_pid "$CLIENT_PID"; CLIENT_PID=""

scenario B3 "kill the surviving relay too (both down): new client fails"
rc=0
if [[ -n "${SURVIVOR_NUM:-}" ]]; then
    stop_relay "$SURVIVOR_NUM"
    expect_connect_failure "$SURVIVOR_URL" || rc=1
else
    rc=1
fi
record B3 "$rc"

scenario B4 "restart both relays: server recovers and a new client connects"
rc=0
start_relay 1
start_relay 2
# The server's relay actors reconnect with backoff; allow several attempts.
connect_and_echo 8 "$RELAY1_URL" "$RELAY2_URL" || rc=1
record B4 "$rc"
kill_pid "$CLIENT_PID"; CLIENT_PID=""

stop_server
stop_relay 1
stop_relay 2

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo
log "Relay failover e2e summary:"
for line in "${SUMMARY[@]}"; do
    note "$line"
done
if [[ "$RESULT" -eq 0 ]]; then
    log "E2E RESULT: ALL PASS ✅"
else
    log "E2E RESULT: FAILURES ❌ (re-run with KEEP_LOGS=1 to inspect $WORK)"
fi
exit "$RESULT"
