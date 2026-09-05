# tunnel-rs end-to-end test scripts

Self-contained end-to-end test that pushes real traffic through a tunnel-rs
tunnel on localhost and verifies it comes back intact.

## What it does

`run_e2e.sh` brings up everything on `127.0.0.1`:

```
echo client (uv/python)                          echo server (uv/python)
        │                                                 ▲
        ▼  127.0.0.1:<target>                             │ 127.0.0.1:<backend>
   tunnel-rs client  ──────── iroh tunnel ─────────  tunnel-rs server
```

For both **TCP** and **UDP** it sends a payload to the tunnel client's local
port and asserts the echo server's reply makes the full round trip.

The script generates the server identity and compact Ed25519 client
authentication keys, uses Python's `json` module to serialize the runtime
configurations, and pipes each configuration directly to `--config-stdin`.
It checks the versioned private-key format, public-key comment, and `0600`
permissions; verifies that an unlisted key is rejected with the authentication
exit code; and confirms that clients sharing one authentication key still use
distinct ephemeral Iroh identities. Authentication key files live only in the
automatically removed temporary working directory. The Python backends and
test clients run through **`uv run`** (PEP 723 inline metadata, no third-party
dependencies).

## Files

| File | Role | Runtime |
|------|------|---------|
| `echo_server.py` | TCP/UDP echo backend | `uv run` |
| `echo_client.py` | Sends a payload, verifies the echo | `uv run` |
| `run_e2e.sh`     | Orchestrator: keygen, configs, processes, assertions | bash |
| `run_relay_failover_e2e.sh` | Relay-only failover scenarios against two local `iroh-relay` instances and a local lookup service | bash |
| `run_lookup_cloudflare_e2e.sh` | The lookup service behind a real Cloudflare quick tunnel, then `run_e2e.sh` against it | bash |
| `lookup_dev.sh` | Sourced helper: a local secret-gated lookup service (`iroh-dns-server` behind `caddy`) and record assertions | bash |

## Requirements

- `uv` and Python ≥ 3.11
- A built `tunnel-rs` binary (the script builds the debug binary if missing)
- **Internet access** for the default run (uses the public iroh relay + the
  default iroh discovery server). Runs with custom relays need no public iroh
  infrastructure, but they need an **address lookup service** (custom relays
  replace n0's lookup with a self-hosted one): either a real one
  (`--lookup-url`/`--lookup-secret`) or `--local-lookup`, which needs
  `iroh-dns-server` (`cargo install iroh-dns-server --version 1.1.0`) and
  `caddy` on the PATH.

## Usage

```bash
./test-scripts/run_e2e.sh
```

With no flags it runs the default test: the public iroh relay plus the default
iroh discovery server (no relay override).

### CLI options

| Flag | Meaning |
|------|---------|
| `--relay-url URL` | Custom relay URL for both sides (**repeatable**). Requires a lookup service (below). Also accepts `--relay-url=URL`. |
| `--lookup-url URL` | Scheme and host of the address lookup service, e.g. a Cloudflare-tunnelled `iroh-dns-server` deployed per [self-hosting.md](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/self-hosting.md). |
| `--lookup-secret S` | The service's `lks1-…` secret. |
| `--local-lookup` | Start a local `iroh-dns-server` behind `caddy` for this run, gated by a fresh secret. |
| `--relay-only` | Force all traffic through the relay, disabling direct P2P. Requires at least one `--relay-url`. |
| `-h`, `--help` | Show help and exit. |

With custom relays the script also asserts the lookup path: the server logs its
first publish, the record is readable through the secret-gated URL and names a
configured relay, and the same record is a 404 without the secret.

Examples:

```bash
# Default: public relay + iroh discovery server (needs internet), no override
./test-scripts/run_e2e.sh

# Your relays and your lookup service (the production layout, e.g. behind
# Cloudflare Tunnels): the run proves the record goes through the tunnel.
./test-scripts/run_e2e.sh --relay-url https://r1.example.com --relay-url https://r2.example.com \
    --lookup-url https://lookup.example.com --lookup-secret lks1-...

# The same, relay-only (no direct P2P)
./test-scripts/run_e2e.sh --relay-url https://relay.example.com --relay-only \
    --lookup-url https://lookup.example.com --lookup-secret lks1-...

# Fully local: a dev relay plus a local lookup service started for the run
./test-scripts/run_e2e.sh --relay-url http://localhost:3340 --relay-only --local-lookup
```

Exit code is `0` when both TCP and UDP round trips pass, non-zero otherwise.

### Running a local relay for offline relay-only tests

`--relay-only` needs a reachable relay. To run fully offline, start a local
`iroh-relay` in dev mode using the bundled config (`relay-dev.toml`, which
disables the metrics server so it won't collide with port 9090):

```bash
# terminal 1: local dev relay on http://localhost:3340
iroh-relay --dev -c test-scripts/relay-dev.toml

# terminal 2: relay-only e2e against it, with a local lookup service
./test-scripts/run_e2e.sh --relay-url http://localhost:3340 --relay-only --local-lookup
```

Install the relay with `cargo install iroh-relay --features server` if you
don't have it. See
[self-hosting.md](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/self-hosting.md)
in iroh-common-architecture for relay ports and config details, including a
production Cloudflare Tunnel setup that serves the relay over a single TCP port
(`relay-prod.toml.example` in this repo is the matching template).

### Environment overrides

| Variable | Default | Meaning |
|----------|---------|---------|
| `TUNNEL_RS_BIN` | `target/debug/tunnel-rs` | Path to the tunnel-rs binary |
| `READY_TIMEOUT` | `60` | Seconds to wait for each process to become ready |
| `KEEP_LOGS` | `0` | Set to `1` to keep the temporary log directory for inspection; it contains no secret-bearing configs |
| `RELAY_URL` | _(unset)_ | Fallback single custom relay, used **only** when no `--relay-url` flag is given (prefer the flag) |
| `LOOKUP_URL`, `LOOKUP_SECRET` | _(unset)_ | Fallbacks for `--lookup-url` / `--lookup-secret` |
| `IROH_DNS_SERVER_BIN`, `CADDY_BIN` | PATH | The `--local-lookup` binaries |

```bash
# Keep per-process logs for debugging
KEEP_LOGS=1 ./test-scripts/run_e2e.sh
```

## Relay failover test

`run_relay_failover_e2e.sh` is a separate, fully offline suite that starts
**two local `iroh-relay --dev` instances** plus a local lookup service
(`iroh-dns-server` behind `caddy`, secret-gated, via `lookup_dev.sh`) and
exercises relay failures in relay-only mode. Servers and clients are each given
an explicit relay list per scenario.

The contract under test is that **startup is strict but runtime is not**: every
configured custom relay is probed individually at startup and all of them must
come online, so a dead relay in the configured set is fatal even when another
relay would work. Once a process is running, losing a relay is survivable and
the peer re-homes onto a surviving one.

- **Phase A (relay down before startup):** a server configured with both relays
  fails to start when either or both are down; a server and client configured
  with only the live relay connect; clients configured with a dead relay (alone
  or alongside a live one) fail to start.
- **Phase B (relay down after startup):** the server's home relay (the one its
  lookup record names) is killed — the running server stays up, re-homes onto
  the survivor (~30s) and **republishes its lookup record naming it**, and a
  restarted client configured with the surviving relay reconnects; with both
  relays killed new clients fail; after both relays restart, clients connect
  again and the record names a live relay.

The record assertions are the point of the lookup service: a server that lost
its relay ends up findable on another one without any client being
reconfigured.

```bash
cargo install iroh-relay --features server            # one-time
cargo install iroh-dns-server --version 1.1.0         # one-time
# plus caddy: https://caddyserver.com/docs/install
./test-scripts/run_relay_failover_e2e.sh
```

Working files and logs go to `./tmp/relay-failover.*` (kept with
`KEEP_LOGS=1`). `TUNNEL_RS_BIN`, `IROH_RELAY_BIN`, and `READY_TIMEOUT` are
honored like in `run_e2e.sh`.

## Lookup service through a real Cloudflare Tunnel

`run_lookup_cloudflare_e2e.sh` reproduces the production path for the lookup
service: a local `iroh-dns-server` behind `caddy`, published through a
throwaway Cloudflare **quick tunnel** (`cloudflared tunnel --url`, no account
needed) as `https://<random>.trycloudflare.com`, and `run_e2e.sh` run
relay-only against a local dev relay with that public URL as the lookup
service. It checks the secret-gated health endpoint answers through Cloudflare
and the ungated one is a 404 there, then the regular e2e asserts the server's
record was published through the tunnel and reads back through it.

```bash
# needs cloudflared, iroh-relay, iroh-dns-server, caddy, and internet access
./test-scripts/run_lookup_cloudflare_e2e.sh
```

To test a permanent deployment instead, run `run_e2e.sh` with your
`--lookup-url` and `--lookup-secret` (see above).

## Running the pieces by hand

The Python helpers are usable on their own:

```bash
uv run test-scripts/echo_server.py --proto tcp --port 9000
uv run test-scripts/echo_client.py --proto tcp --port 9000 --message hi
```
