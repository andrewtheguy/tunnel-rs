#!/usr/bin/env bash
#
# Relay failover end-to-end test for tunnel-rs (relay-only mode, no internet).
#
# Runs TWO local iroh-relay instances (`--dev` mode, plain HTTP) and exercises
# relay failure scenarios. Servers and clients are each configured with an
# explicit relay list per scenario. Custom relays disable internet discovery
# automatically, so the whole test runs without any public iroh infrastructure.
#
# Contract under test: a custom relay set holds at least TWO relays, because a
# server rides out a relay outage by moving onto another configured relay in
# place (no rebuild, no dropped identity or connections). Every configured
# relay is probed individually at startup; a relay that is down is a warning,
# and startup fails only when none is reachable. Clients dial with every
# configured relay as a hint, so a server homed on any of them is reachable.
#
# Phase A - relay offline BEFORE startup (the per-relay startup probe):
#   A0  both relays down; server configured with both ..... startup fails (negative)
#   A1  only relay2 up; server and client with both ....... both start (warning
#       for relay1); the client connects via relay2; TCP echo passes
#   A2  a single custom relay is rejected as configuration: server and client
#       configured with ONLY relay2 ........................ startup fails (negative)
#
# Phase B - a relay dies AFTER startup (iroh's own re-homing):
#   B1  both relays up; server and client with both relays; connects; TCP echo passes
#   B2  the server's home relay is killed; the server stays up and re-homes onto
#       the survivor on its own (net_report re-probes every ~20-26s); a restarted
#       client configured with both relays connects; TCP echo passes
#   B3  the surviving relay is killed too (both down); a new client fails (negative)
#   B4  both relays are restarted; a new client with both relays connects again
#       (server relay reconnect + re-home); TCP echo passes
#
# Phase C - the home relay stays "healthy" for net_report but cannot be connected
#           (the shared in-place home-relay failover, flexaccess_iroh::relay_failover):
#   C0  relay1 direct, relay2 behind a proxy that adds latency; the server homes
#       on relay1 deterministically; a client connects; TCP echo passes
#   C1  relay1 is replaced, on the same port, by a fake that answers the net-report
#       probe (/ping) but refuses relay connections. iroh keeps preferring it, so
#       nothing re-homes on its own; after 60s the failover removes it from the
#       relay map, the forced net report homes the server on relay2, and a new
#       client connects through relay2; TCP echo passes. The server process and
#       its endpoint never restart.
#   C2  the real relay1 comes back on its port; the failover's restore probe puts
#       it back in the relay map (checked every 90s), the server moves back onto
#       it, and a client connects via relay1; TCP echo passes
#
# Requirements: iroh-relay (cargo install iroh-relay), uv, python3.
#
# Usage:
#   ./run_relay_failover_e2e.sh
#
# Environment overrides:
#   TUNNEL_RS_BIN   path to the tunnel-rs binary (default: cargo-built debug binary)
#   IROH_RELAY_BIN  path to the iroh-relay binary (default: iroh-relay on PATH)
#   FLEXACCESS_KEYS_BIN
#                   path to the flexaccess-keys binary used for client
#                   authentication keys (default: PATH, then a download of the
#                   pinned release)
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
PROXY_PORT="$(pick_port)"
DEAD_PORT="$(pick_port)"
BACKEND_PORT="$(pick_port)"
RELAY1_URL="http://127.0.0.1:$RELAY1_PORT"
RELAY2_URL="http://127.0.0.1:$RELAY2_PORT"
# relay2 as seen through the latency-adding proxy (phase C).
PROXY_URL="http://127.0.0.1:$PROXY_PORT"
# A relay URL nothing ever listens on: fills the second slot of a client that
# must only use one live relay, since a single custom relay is rejected.
DEAD_URL="http://127.0.0.1:$DEAD_PORT"
log "Relays: relay1=$RELAY1_URL relay2=$RELAY2_URL proxy->relay2=$PROXY_URL backend=127.0.0.1:$BACKEND_PORT"

read -r ENDPOINT_ID SECRET < <(
    "$BIN" generate-server-key --json |
        python3 -c 'import json, sys; value = json.load(sys.stdin); print(value["public_key"], value["private_key"])'
)
source "$SCRIPT_DIR/flexaccess_keys_bin.sh"
resolve_flexaccess_keys_bin "$WORK"
"$KEYS_BIN" generate-auth-key "failover e2e client" \
    > "$WORK/client.key"
"$KEYS_BIN" show-auth-key --private-key-file "$WORK/client.key" > "$WORK/authorized_keys"
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

# A relay that answers the net-report probe but refuses relay connections, on
# relay1's port (see fake_relay.py).
FAKE_RELAY_PID=""
start_fake_relay() {
    local logfile="$WORK/fake_relay.$(date +%s%N).log"
    setsid python3 "$SCRIPT_DIR/fake_relay.py" --port "$RELAY1_PORT" >"$logfile" 2>&1 &
    FAKE_RELAY_PID=$!
    PIDS+=("$FAKE_RELAY_PID")
    wait_for_log "$logfile" "READY fake relay" 30 || {
        echo "ERROR: fake relay did not start; log:" >&2
        cat "$logfile" >&2
        return 1
    }
    note "fake relay up on relay1's port $RELAY1_PORT (pid $FAKE_RELAY_PID)"
}

stop_fake_relay() {
    kill_pid "$FAKE_RELAY_PID"
    FAKE_RELAY_PID=""
    note "fake relay stopped"
}

# relay2 behind a proxy that delays each new connection, so net_report always
# measures it as the slower relay (see delay_proxy.py).
PROXY_PID=""
PROXY_DELAY_MS=40
start_delay_proxy() {
    local logfile="$WORK/delay_proxy.$(date +%s%N).log"
    setsid python3 "$SCRIPT_DIR/delay_proxy.py" --listen "$PROXY_PORT" \
        --upstream "$RELAY2_PORT" --delay-ms "$PROXY_DELAY_MS" >"$logfile" 2>&1 &
    PROXY_PID=$!
    PIDS+=("$PROXY_PID")
    wait_for_log "$logfile" "READY delay proxy" 30 || {
        echo "ERROR: delay proxy did not start; log:" >&2
        cat "$logfile" >&2
        return 1
    }
    note "delay proxy up: $PROXY_URL -> relay2 (+${PROXY_DELAY_MS}ms per connection)"
}

stop_delay_proxy() {
    kill_pid "$PROXY_PID"
    PROXY_PID=""
    note "delay proxy stopped"
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
print(json.dumps({"role": "server", "iroh": iroh}))
' "$WORK/authorized_keys" "$@"
    )"
    printf '%s\n' "$config" |
        setsid "$BIN" server --config-stdin --relay-only >"$SERVER_LOG" 2>&1 &
    SERVER_PID=$!
    PIDS+=("$SERVER_PID")
}

# Start a server that is EXPECTED to fail at startup for the reason matching
# regex $1. Passes when the process reports that failure and never becomes
# ready. Args: <failure_regex> <relay_url>...
expect_server_start_failure() {
    local pattern="$1"; shift
    local rc=0
    start_server "$@"
    wait_for_log_or_death "$SERVER_PID" "$SERVER_LOG" \
        "$pattern" "$READY_TIMEOUT" || rc=$?
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

# The port of the relay the server last reported as its connected home relay.
# The shared failover logs "Home relay: <url> connected" on a change while
# healthy and "Home relay connection restored on <url> after Ns" when an
# outage ends; either names the current home relay.
server_home_relay_port() {
    grep -Eo "Home relay: http://127\.0\.0\.1:[0-9]+/ connected|Home relay connection restored on http://127\.0\.0\.1:[0-9]+/" "$SERVER_LOG" |
        tail -1 | sed -E 's|.*:([0-9]+)/.*|\1|'
}

# Wait until the server reports its home relay connected on port $1.
wait_for_home_relay() {
    local port="$1" timeout="$2"
    local max_attempts=$(( timeout * 2 )) attempt=0
    while (( attempt < max_attempts )); do
        [[ "$(server_home_relay_port)" == "$port" ]] && return 0
        sleep 0.5
        attempt=$(( attempt + 1 ))
    done
    return 1
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
print(json.dumps({"role": "client", "iroh": iroh}))
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
# a relay failure the server needs a re-probe cycle (~20-26s) to re-home, and a
# relay-only dial through a dead relay takes 10s to time out before the next
# relay is tried.
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

# The message the startup relay probe emits when EVERY configured relay is
# down (one dead relay is only a warning now). Negative scenarios must fail in
# the expected way; requiring the message keeps an unrelated startup failure
# (bad key, port in use, malformed config) from passing as the expected one.
RELAY_PROBE_FAILURE='all [0-9]+ custom relays failed to come online'
# The config error for fewer than two custom relays.
SINGLE_RELAY_REJECTED='at least 2 distinct relay_urls'

# Start a client that is EXPECTED to fail to connect for the reason matching
# regex $1. Passes when the process exits without ever establishing the tunnel
# AND reports that failure. Args: <failure_regex> <relay_url>...
expect_connect_failure() {
    local pattern="$1"; shift
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
    if [[ "$rc" -ne 2 ]]; then
        note "client neither connected nor exited within ${READY_TIMEOUT}s"
        return 1
    fi
    # It exited without connecting - make sure it exited for the expected reason.
    if ! grep -Eq "$pattern" "$logfile"; then
        note "client exited without the expected failure ($pattern); log:"
        cat "$logfile" >&2
        return 1
    fi
    return 0
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
expect_server_start_failure "$RELAY_PROBE_FAILURE" "$RELAY1_URL" "$RELAY2_URL" || rc=1
record A0 "$rc"

scenario A1 "relay1 down: server and client configured with both start on relay2 alone"
start_relay 2
rc=0
expect_server_ready "$RELAY1_URL" "$RELAY2_URL" || rc=1
if [[ "$rc" -eq 0 ]] && ! grep -Eq "1 of 2 custom relays failed to come online" "$SERVER_LOG"; then
    note "server did not warn about the dead relay"
    rc=1
fi
if [[ "$rc" -eq 0 ]]; then
    # relay1 is dialed first and refused at once; the dial falls through to relay2.
    connect_and_echo 3 "$RELAY1_URL" "$RELAY2_URL" || rc=1
fi
if [[ "$rc" -eq 0 ]] && ! grep -Eq "1 of 2 custom relays failed to come online" "$CLIENT_LOG"; then
    note "client did not warn about the dead relay"
    rc=1
fi
record A1 "$rc"
kill_pid "$CLIENT_PID"; CLIENT_PID=""
stop_server

scenario A2 "a single custom relay is rejected: server and client with only relay2 fail to start"
rc=0
expect_server_start_failure "$SINGLE_RELAY_REJECTED" "$RELAY2_URL" || rc=1
expect_connect_failure "$SINGLE_RELAY_REJECTED" "$RELAY2_URL" || rc=1
record A2 "$rc"

stop_relay 2

# ===========================================================================
# Phase B - a relay dies AFTER client/server are connected (iroh re-homes)
# ===========================================================================

scenario B1 "both relays up: client with both relays connects"
start_relay 1
start_relay 2
rc=0
expect_server_ready "$RELAY1_URL" "$RELAY2_URL" || rc=1
HOME_RELAY_NUM=""
if [[ "$rc" -eq 0 ]]; then
    connect_and_echo 3 "$RELAY1_URL" "$RELAY2_URL" || rc=1
fi
if [[ "$rc" -eq 0 ]]; then
    case "$(server_home_relay_port)" in
        "$RELAY1_PORT") HOME_RELAY_NUM=1 ;;
        "$RELAY2_PORT") HOME_RELAY_NUM=2 ;;
        *) note "server did not report a connected home relay"; rc=1 ;;
    esac
    note "server home relay is relay$HOME_RELAY_NUM"
fi
record B1 "$rc"

scenario B2 "kill the server's home relay: server re-homes on its own, a restarted client connects via the survivor"
rc=0
if [[ -n "$HOME_RELAY_NUM" ]]; then
    SURVIVOR_NUM=$(( 3 - HOME_RELAY_NUM ))
    SURVIVOR_PORT="$(eval echo "\$RELAY${SURVIVOR_NUM}_PORT")"
    stop_relay "$HOME_RELAY_NUM"
    # The old client's QUIC connection lingers until it times out; restart the
    # client instead (real deployments restart via a supervisor).
    kill_pid "$CLIENT_PID"; CLIENT_PID=""
    # Losing a relay at runtime must NOT take the already-started server down.
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        note "server exited when a relay died at runtime"
        rc=1
    fi
    # The dead relay stops answering net_report's probe, so iroh itself moves
    # the home relay within a re-probe cycle, no failover action needed.
    if [[ "$rc" -eq 0 ]]; then
        wait_for_home_relay "$SURVIVOR_PORT" 60 || { note "server did not re-home onto relay$SURVIVOR_NUM"; rc=1; }
    fi
    if [[ "$rc" -eq 0 ]] && grep -Eq "Removed .* from the relay map" "$SERVER_LOG"; then
        note "the failover acted although iroh re-homed on its own"
        rc=1
    fi
    # The restarted client lists both relays (one is dead: a warning, and a
    # 10s dial timeout before the survivor is tried).
    if [[ "$rc" -eq 0 ]]; then
        connect_and_echo 4 "$RELAY1_URL" "$RELAY2_URL" || rc=1
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
    expect_connect_failure "$RELAY_PROBE_FAILURE" "$RELAY1_URL" "$RELAY2_URL" || rc=1
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

# ===========================================================================
# Phase C - the home relay answers probes but cannot be connected (in-place failover)
# ===========================================================================

scenario C0 "relay1 direct, relay2 behind a slow proxy: server homes on relay1"
rc=0
start_relay 1
start_relay 2
start_delay_proxy
expect_server_ready "$RELAY1_URL" "$PROXY_URL" || rc=1
if [[ "$rc" -eq 0 ]]; then
    wait_for_home_relay "$RELAY1_PORT" 30 || { note "server did not home on relay1"; rc=1; }
fi
if [[ "$rc" -eq 0 ]]; then
    connect_and_echo 3 "$RELAY1_URL" "$PROXY_URL" || rc=1
fi
record C0 "$rc"
kill_pid "$CLIENT_PID"; CLIENT_PID=""

scenario C1 "relay1 becomes a fake that answers probes but refuses connections: the failover moves the server onto relay2 in place"
rc=0
if [[ "$rc" -eq 0 ]]; then
    stop_relay 1
    start_fake_relay || rc=1
fi
if [[ "$rc" -eq 0 ]]; then
    wait_for_log "$SERVER_LOG" "No connected home relay" 60 || { note "server never noticed the relay loss"; rc=1; }
fi
if [[ "$rc" -eq 0 ]]; then
    # 60s outage window, then the wedged relay is taken out of the relay map.
    wait_for_log "$SERVER_LOG" "Removed $RELAY1_URL/ from the relay map" 90 || {
        note "the failover did not remove relay1 from the relay map"; rc=1; }
fi
if [[ "$rc" -eq 0 ]]; then
    wait_for_log "$SERVER_LOG" "Home relay connection restored on $PROXY_URL/" 60 || {
        note "server did not home on relay2 after the failover"; rc=1; }
fi
if [[ "$rc" -eq 0 ]]; then
    # iroh must not have re-homed on its own before the failover acted.
    removed_line="$(grep -En "Removed $RELAY1_URL/ from the relay map" "$SERVER_LOG" | head -1 | cut -d: -f1)"
    restored_line="$(grep -En "Home relay connection restored on $PROXY_URL/" "$SERVER_LOG" | head -1 | cut -d: -f1)"
    if (( restored_line < removed_line )); then
        note "server re-homed before the failover acted (lines $restored_line < $removed_line)"
        rc=1
    fi
fi
if [[ "$rc" -eq 0 ]] && ! kill -0 "$SERVER_PID" 2>/dev/null; then
    note "server process died"
    rc=1
fi
if [[ "$rc" -eq 0 ]] && grep -Eq "Endpoint rebuilt|rebuilding the endpoint" "$SERVER_LOG"; then
    note "server rebuilt its endpoint; the failover must act in place"
    rc=1
fi
# A new client reaches the server through relay2. Its second relay slot is a
# dead port rather than the fake: a client that homed on the fake could never
# come online, and the failover runs only on the server.
if [[ "$rc" -eq 0 ]]; then
    connect_and_echo 3 "$PROXY_URL" "$DEAD_URL" || rc=1
fi
record C1 "$rc"
kill_pid "$CLIENT_PID"; CLIENT_PID=""

scenario C2 "the real relay1 returns: the restore probe puts it back and the server moves back onto it"
rc=0
stop_fake_relay
start_relay 1
# The restore probe runs every 90s after the removal; allow for one full
# interval plus the 10s probe, then a net-report cycle for the move back.
if [[ "$rc" -eq 0 ]]; then
    wait_for_log "$SERVER_LOG" "$RELAY1_URL/ is connectable again and back in the relay map" 150 || {
        note "relay1 was not restored to the relay map"; rc=1; }
fi
if [[ "$rc" -eq 0 ]]; then
    wait_for_home_relay "$RELAY1_PORT" 60 || { note "server did not move back onto relay1"; rc=1; }
fi
if [[ "$rc" -eq 0 ]]; then
    connect_and_echo 3 "$RELAY1_URL" "$PROXY_URL" || rc=1
fi
record C2 "$rc"
kill_pid "$CLIENT_PID"; CLIENT_PID=""

stop_server
stop_delay_proxy
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
