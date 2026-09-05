#!/usr/bin/env bash
#
# Sourced helper: a LOCAL address lookup service for the e2e scripts.
#
# Custom relays require one (see relays-and-address-lookup.md in
# iroh-common-architecture). This starts the same stack self-hosting.md
# documents, on localhost with a fresh secret:
#
#   tunnel-rs  --->  caddy :$CADDY_PORT  --->  iroh-dns-server :$DNS_HTTP_PORT
#                    handle_path /<secret>/*       PUT/GET /pkarr/<id>
#                    everything else: 404
#
# Provides (all need python3):
#   start_local_lookup <workdir> <tunnel-rs-bin>
#       Starts both processes (their PIDs go into the caller's PIDS array),
#       waits until the gate answers, exports LOOKUP_URL and LOOKUP_SECRET.
#   lookup_record_status <endpoint-id>
#       HTTP status of the endpoint's record through the gate (200 = published).
#   lookup_record_names_relay <endpoint-id> <relay-url>
#       0 when the published record carries `relay=<relay-url>`.
#   wait_for_lookup_record <endpoint-id> <relay-url> <timeout-seconds>
#       Polls lookup_record_names_relay.
#   http_status <url>
#       HTTP status code (0 when unreachable).
#
# Environment overrides:
#   IROH_DNS_SERVER_BIN  path to iroh-dns-server (default: PATH)
#   CADDY_BIN            path to caddy (default: PATH)
#
# Install: cargo install iroh-dns-server --version 1.1.0
#          https://caddyserver.com/docs/install

lookup_require_bins() {
    DNS_SERVER_BIN="${IROH_DNS_SERVER_BIN:-$(command -v iroh-dns-server || true)}"
    CADDY_BIN="${CADDY_BIN:-$(command -v caddy || true)}"
    if [[ -z "$DNS_SERVER_BIN" || ! -x "$DNS_SERVER_BIN" ]]; then
        echo "ERROR: iroh-dns-server not found. Install with: cargo install iroh-dns-server --version 1.1.0" >&2
        return 1
    fi
    if [[ -z "$CADDY_BIN" || ! -x "$CADDY_BIN" ]]; then
        echo "ERROR: caddy not found. See https://caddyserver.com/docs/install" >&2
        return 1
    fi
}

# A free localhost port; $1 is "tcp" (default) or "udp".
lookup_pick_port() {
    python3 - "${1:-tcp}" <<'PY'
import socket, sys
kind = socket.SOCK_DGRAM if sys.argv[1] == "udp" else socket.SOCK_STREAM
s = socket.socket(socket.AF_INET, kind)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

http_status() {
    python3 - "$1" <<'PY'
import sys, urllib.error, urllib.request
try:
    with urllib.request.urlopen(sys.argv[1], timeout=5) as response:
        print(response.status)
except urllib.error.HTTPError as error:
    print(error.code)
except Exception:
    print(0)
PY
}

# iroh prints endpoint ids as hex; the pkarr path uses z-base-32.
endpoint_id_z32() {
    python3 - "$1" <<'PY'
import sys
raw = bytes.fromhex(sys.argv[1])
alphabet = "ybndrfg8ejkmcpqxot1uwisza345h769"
buffer = bits = 0
out = []
for byte in raw:
    buffer = (buffer << 8) | byte
    bits += 8
    while bits >= 5:
        bits -= 5
        out.append(alphabet[(buffer >> bits) & 31])
if bits:
    out.append(alphabet[(buffer << (5 - bits)) & 31])
print("".join(out))
PY
}

lookup_record_url() {
    echo "$LOOKUP_URL/$LOOKUP_SECRET/pkarr/$(endpoint_id_z32 "$1")"
}

lookup_record_status() {
    http_status "$(lookup_record_url "$1")"
}

lookup_record_names_relay() {
    python3 - "$(lookup_record_url "$1")" "$2" <<'PY'
import sys, urllib.request
try:
    with urllib.request.urlopen(sys.argv[1], timeout=5) as response:
        body = response.read()
except Exception:
    sys.exit(1)
# A pkarr relay payload: 64-byte signature, 8-byte timestamp, then the DNS
# packet, whose TXT record carries `relay=<url>` verbatim (iroh normalizes
# relay URLs with a trailing slash).
needle = ("relay=" + sys.argv[2].rstrip("/") + "/").encode()
sys.exit(0 if needle in body else 1)
PY
}

wait_for_lookup_record() {
    local endpoint_id="$1" relay_url="$2" timeout="$3" attempt=0
    while (( attempt < timeout )); do
        if lookup_record_names_relay "$endpoint_id" "$relay_url"; then
            return 0
        fi
        sleep 1
        attempt=$(( attempt + 1 ))
    done
    return 1
}

start_local_lookup() {
    local work="$1" bin="$2"
    lookup_require_bins || return 1
    local dns_http_port dns_udp_port caddy_port
    dns_http_port="$(lookup_pick_port)"
    dns_udp_port="$(lookup_pick_port udp)"
    caddy_port="$(lookup_pick_port)"
    LOOKUP_SECRET="$("$bin" generate-lookup-secret)"
    mkdir -p "$work/lookup"

    # The documented dns.toml, on ephemeral ports. Top-level keys first: in
    # TOML a key after a [table] header belongs to that table.
    cat > "$work/lookup/dns.toml" <<TOML
pkarr_put_rate_limit = "disabled"
data_dir = "$work/lookup/data"

[http]
port = $dns_http_port
bind_addr = "127.0.0.1"

[dns]
port = $dns_udp_port
bind_addr = "127.0.0.1"
default_ttl = 30
# The root origin (".") is required: iroh-dns-server keeps its static zone
# there and refuses to start without an SOA for it.
origins = ["lookup.test", "."]
default_soa = "ns1.lookup.test hostmaster.lookup.test 0 10800 3600 604800 3600"

[mainline]
enabled = false

[metrics]
disabled = true
TOML

    # The documented Caddy block: the secret prefix is stripped on the way to
    # iroh-dns-server and every other path is a 404. The site address is a
    # bare port on purpose: Caddy matches sites on the Host header, and a
    # tunnel (Cloudflare) forwards the public hostname, not 127.0.0.1.
    cat > "$work/lookup/Caddyfile" <<CADDY
{
	admin off
	auto_https off
}

:$caddy_port {
	bind 127.0.0.1
	handle_path /$LOOKUP_SECRET/* {
		reverse_proxy 127.0.0.1:$dns_http_port
	}
	respond 404
}
CADDY

    setsid "$DNS_SERVER_BIN" --config "$work/lookup/dns.toml" \
        > "$work/lookup/dns-server.log" 2>&1 &
    PIDS+=("$!")
    setsid "$CADDY_BIN" run --config "$work/lookup/Caddyfile" --adapter caddyfile \
        > "$work/lookup/caddy.log" 2>&1 &
    PIDS+=("$!")

    LOOKUP_URL="http://127.0.0.1:$caddy_port"
    local attempt=0
    while [[ "$(http_status "$LOOKUP_URL/$LOOKUP_SECRET/healthz")" != "200" ]]; do
        if (( attempt >= 60 )); then
            echo "ERROR: local lookup service did not come up; logs:" >&2
            cat "$work/lookup/dns-server.log" "$work/lookup/caddy.log" >&2 || true
            return 1
        fi
        sleep 0.5
        attempt=$(( attempt + 1 ))
    done
    if [[ "$(http_status "$LOOKUP_URL/healthz")" != "404" ]]; then
        echo "ERROR: the lookup gate answers without the secret" >&2
        return 1
    fi
    export LOOKUP_URL LOOKUP_SECRET
    echo "==> Local lookup service: $LOOKUP_URL (caddy, secret-gated) -> iroh-dns-server :$dns_http_port"
}
