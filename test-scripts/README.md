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

The script generates the server identity and a compact Ed25519 client
authentication key, uses Python's `json` module to serialize the runtime
configurations, and pipes each configuration directly to `--config-stdin`.
Authentication key files live only in the automatically removed temporary
working directory. The Python backends and test clients run through
**`uv run`** (PEP 723 inline metadata, no third-party dependencies).

## Files

| File | Role | Runtime |
|------|------|---------|
| `echo_server.py` | TCP/UDP echo backend | `uv run` |
| `echo_client.py` | Sends a payload, verifies the echo | `uv run` |
| `run_e2e.sh`     | Orchestrator: keygen, configs, processes, assertions | bash |
| `run_relay_failover_e2e.sh` | Relay-only failover scenarios against two local `iroh-relay` instances | bash |

## Requirements

- `uv` and Python ≥ 3.11
- A built `tunnel-rs` binary (the script builds the debug binary if missing)
- **Internet access** for the default run (uses the public iroh relay + the
  default iroh discovery server). Runs with custom relays disable internet
  discovery automatically and need no public iroh infrastructure.

## Usage

```bash
./test-scripts/run_e2e.sh
```

With no flags it runs the default test: the public iroh relay plus the default
iroh discovery server (no relay override).

### CLI options

| Flag | Meaning |
|------|---------|
| `--relay-url URL` | Custom relay URL for both sides (**repeatable**). Custom relays disable internet discovery automatically; clients reach the server through relay hints. Also accepts `--relay-url=URL`. |
| `--relay-only` | Force all traffic through the relay, disabling direct P2P. Requires at least one `--relay-url`. |
| `-h`, `--help` | Show help and exit. |

Examples:

```bash
# Default: public relay + iroh discovery server (needs internet), no override
./test-scripts/run_e2e.sh

# One custom relay (internet discovery auto-disabled)
./test-scripts/run_e2e.sh --relay-url https://relay.example.com

# Multiple relays (failover; internet discovery auto-disabled)
./test-scripts/run_e2e.sh --relay-url https://r1.example.com --relay-url https://r2.example.com

# Relay-only e2e (no direct P2P; requires a custom relay)
./test-scripts/run_e2e.sh --relay-url https://relay.example.com --relay-only
```

Exit code is `0` when both TCP and UDP round trips pass, non-zero otherwise.

### Running a local relay for offline relay-only tests

`--relay-only` needs a reachable relay. To run fully offline, start a local
`iroh-relay` in dev mode using the bundled config (`relay-dev.toml`, which
disables the metrics server so it won't collide with port 9090):

```bash
# terminal 1: local dev relay on http://localhost:3340
iroh-relay --dev -c test-scripts/relay-dev.toml

# terminal 2: relay-only e2e against it
./test-scripts/run_e2e.sh --relay-url http://localhost:3340 --relay-only
```

Install the relay with `cargo install iroh-relay --features server` if you
don't have it. See
[`../docs/SELF-HOSTING.md`](../docs/SELF-HOSTING.md) for relay ports and config
details, including a production `relay-prod.toml.example` + Cloudflare Tunnel
setup that serves the relay over a single TCP port.

### Environment overrides

| Variable | Default | Meaning |
|----------|---------|---------|
| `TUNNEL_RS_BIN` | `target/debug/tunnel-rs` | Path to the tunnel-rs binary |
| `READY_TIMEOUT` | `60` | Seconds to wait for each process to become ready |
| `KEEP_LOGS` | `0` | Set to `1` to keep the temporary log directory for inspection; it contains no secret-bearing configs |
| `RELAY_URL` | _(unset)_ | Fallback single custom relay, used **only** when no `--relay-url` flag is given (prefer the flag) |

```bash
# Keep per-process logs for debugging
KEEP_LOGS=1 ./test-scripts/run_e2e.sh
```

## Relay failover test

`run_relay_failover_e2e.sh` is a separate, fully offline suite that starts
**two local `iroh-relay --dev` instances** and exercises relay failures in
relay-only mode. The tunnel server is always configured with both relays;
clients run with both or with only one of them:

- **Phase A (relay down before connecting):** server fails cleanly with no
  relays up; with one relay down, clients connect via the live one (configured
  with both relays or only the live one); a client configured with only the
  dead relay fails.
- **Phase B (relay down after connecting):** the relay carrying the connection
  is killed and a restarted client fails over to the surviving relay (the
  server re-homes within ~30s); with both relays killed new clients fail; after
  both relays restart, clients connect again.

```bash
cargo install iroh-relay --features server   # one-time
./test-scripts/run_relay_failover_e2e.sh
```

Working files and logs go to `./tmp/relay-failover.*` (kept with
`KEEP_LOGS=1`). `TUNNEL_RS_BIN`, `IROH_RELAY_BIN`, and `READY_TIMEOUT` are
honored like in `run_e2e.sh`.

## Running the pieces by hand

The Python helpers are usable on their own:

```bash
uv run test-scripts/echo_server.py --proto tcp --port 9000
uv run test-scripts/echo_client.py --proto tcp --port 9000 --message hi
```
