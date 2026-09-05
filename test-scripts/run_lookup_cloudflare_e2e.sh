#!/usr/bin/env bash
#
# The address lookup service behind a REAL Cloudflare Tunnel, end to end.
#
# self-hosting.md deploys the lookup service (iroh-dns-server behind caddy)
# behind a Cloudflare Tunnel. This script reproduces that path with a
# throwaway *quick tunnel* (`cloudflared tunnel --url`, no account needed):
#
#   tunnel-rs ---> https://<random>.trycloudflare.com ---> cloudflared
#             ---> caddy (strips /<secret>) ---> iroh-dns-server
#
# and runs run_e2e.sh in relay-only mode against a local `iroh-relay --dev`
# with that public URL as the lookup service. The server's record must be
# published through Cloudflare and readable back through it, gated by the
# secret; then TCP and UDP round trips must pass.
#
# Requirements: cloudflared, iroh-relay, iroh-dns-server, caddy, uv, python3,
# and internet access (Cloudflare's edge).
#
# Usage:
#   ./run_lookup_cloudflare_e2e.sh
#
# Environment overrides: TUNNEL_RS_BIN, IROH_RELAY_BIN, IROH_DNS_SERVER_BIN,
# CADDY_BIN, CLOUDFLARED_BIN, KEEP_LOGS, READY_TIMEOUT (as in run_e2e.sh).
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

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
export TUNNEL_RS_BIN="$BIN"

RELAY_BIN="${IROH_RELAY_BIN:-$(command -v iroh-relay || true)}"
[[ -n "$RELAY_BIN" && -x "$RELAY_BIN" ]] || {
    echo "ERROR: iroh-relay not found. Install with: cargo install iroh-relay" >&2
    exit 1
}
CLOUDFLARED_BIN="${CLOUDFLARED_BIN:-$(command -v cloudflared || true)}"
[[ -n "$CLOUDFLARED_BIN" && -x "$CLOUDFLARED_BIN" ]] || {
    echo "ERROR: cloudflared not found. See https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/" >&2
    exit 1
}
source "$SCRIPT_DIR/lookup_dev.sh"
lookup_require_bins

WORK="$(mktemp -d)"
declare -a PIDS=()

log() { printf '==> %s\n' "$*"; }

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

# ---------------------------------------------------------------------------
# Local relay (dev mode, plain HTTP)
# ---------------------------------------------------------------------------
RELAY_PORT="$(lookup_pick_port)"
cat > "$WORK/relay.toml" <<TOML
enable_metrics = false
http_bind_addr = "127.0.0.1:$RELAY_PORT"
TOML
setsid "$RELAY_BIN" --dev -c "$WORK/relay.toml" > "$WORK/relay.log" 2>&1 &
PIDS+=("$!")
RELAY_URL="http://127.0.0.1:$RELAY_PORT"
for _ in $(seq 1 60); do
    [[ "$(http_status "$RELAY_URL/")" != "0" ]] && break
    sleep 0.5
done
log "Local relay: $RELAY_URL"

# ---------------------------------------------------------------------------
# Lookup service (caddy + iroh-dns-server) and the Cloudflare quick tunnel
# ---------------------------------------------------------------------------
start_local_lookup "$WORK" "$BIN"
LOCAL_LOOKUP_URL="$LOOKUP_URL"

log "Opening a Cloudflare quick tunnel to $LOCAL_LOOKUP_URL..."
setsid "$CLOUDFLARED_BIN" tunnel --no-autoupdate --url "$LOCAL_LOOKUP_URL" \
    > "$WORK/cloudflared.log" 2>&1 &
PIDS+=("$!")
PUBLIC_URL=""
for _ in $(seq 1 120); do
    PUBLIC_URL="$(grep -Eo 'https://[a-z0-9-]+\.trycloudflare\.com' "$WORK/cloudflared.log" | head -1 || true)"
    [[ -n "$PUBLIC_URL" ]] && break
    sleep 0.5
done
if [[ -z "$PUBLIC_URL" ]]; then
    echo "ERROR: cloudflared did not report a trycloudflare.com URL; log:" >&2
    cat "$WORK/cloudflared.log" >&2 || true
    exit 1
fi
log "Cloudflare quick tunnel: $PUBLIC_URL"

# The tunnel's DNS name takes a moment to appear. Do NOT ask the system
# resolver for it until it exists: a resolver that sees the name before it is
# published caches the NXDOMAIN for the zone's negative TTL (30 minutes for
# trycloudflare.com), and every later query - including tunnel-rs's - fails
# for that long. So first confirm the name through Cloudflare's own resolver
# over DNS-over-HTTPS, which bypasses every cache in between.
PUBLIC_HOST="${PUBLIC_URL#https://}"
published=0
for _ in $(seq 1 120); do
    if python3 - "$PUBLIC_HOST" <<'PY'
import json, sys, urllib.request
request = urllib.request.Request(
    f"https://cloudflare-dns.com/dns-query?name={sys.argv[1]}&type=A",
    headers={"accept": "application/dns-json"},
)
try:
    with urllib.request.urlopen(request, timeout=10) as response:
        answer = json.load(response)
except Exception:
    sys.exit(1)
sys.exit(0 if answer.get("Status") == 0 and answer.get("Answer") else 1)
PY
    then
        published=1
        break
    fi
    sleep 2
done
if [[ "$published" != "1" ]]; then
    echo "ERROR: $PUBLIC_HOST never appeared in Cloudflare DNS; cloudflared log:" >&2
    cat "$WORK/cloudflared.log" >&2 || true
    exit 1
fi
log "Quick tunnel name published in Cloudflare DNS"

# Now the system resolver (what tunnel-rs uses) may be asked: wait until the
# gated health check answers through Cloudflare, and confirm the gate holds
# there.
ready=0
for _ in $(seq 1 120); do
    if [[ "$(http_status "$PUBLIC_URL/$LOOKUP_SECRET/healthz")" == "200" ]]; then
        ready=1
        break
    fi
    sleep 1
done
if [[ "$ready" != "1" ]]; then
    echo "ERROR: lookup service not reachable through the Cloudflare tunnel; cloudflared log:" >&2
    cat "$WORK/cloudflared.log" >&2 || true
    exit 1
fi
if [[ "$(http_status "$PUBLIC_URL/healthz")" != "404" ]]; then
    echo "ERROR: the lookup gate answers through the tunnel without the secret" >&2
    exit 1
fi
log "Lookup service reachable through Cloudflare, secret-gated: PASS"

# ---------------------------------------------------------------------------
# The regular e2e, with the PUBLIC lookup URL
# ---------------------------------------------------------------------------
log "Running run_e2e.sh (relay-only, lookup via Cloudflare)..."
"$SCRIPT_DIR/run_e2e.sh" --relay-url "$RELAY_URL" --relay-only \
    --lookup-url "$PUBLIC_URL" --lookup-secret "$LOOKUP_SECRET"
