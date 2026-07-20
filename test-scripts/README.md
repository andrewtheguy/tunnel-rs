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

The script generates the identity and auth token with the commands' `--json`
mode, uses Python's `json` module to parse those results and serialize the
runtime configurations, and pipes each configuration directly to
`--config-stdin`. No key, token, or secret-bearing config is written to the
temporary working directory. The Python backends and test clients run through
**`uv run`** (PEP 723 inline metadata, no third-party dependencies).

## Files

| File | Role | Runtime |
|------|------|---------|
| `echo_server.py` | TCP/UDP echo backend | `uv run` |
| `echo_client.py` | Sends a payload, verifies the echo | `uv run` |
| `run_e2e.sh`     | Orchestrator: keygen, configs, processes, assertions | bash |

## Requirements

- `uv` and Python ≥ 3.11
- A built `tunnel-rs` binary (the script builds the debug binary if missing)
- **Internet access** for the default run (uses the public iroh relay + the
  default iroh discovery server), and for runs with multiple custom relays.
  A run with one custom relay sets `discovery = "none"` and needs no public
  iroh infrastructure.

## Usage

```bash
./test-scripts/run_e2e.sh
```

With no flags it runs the default test: the public iroh relay plus the default
iroh discovery server (no relay override).

### CLI options

| Flag | Meaning |
|------|---------|
| `--relay-url URL` | Custom relay URL for both sides (**repeatable**). One relay sets `discovery = "none"`; multiple relays retain public discovery so clients can locate the server's home relay. Also accepts `--relay-url=URL`. |
| `--relay-only` | Force all traffic through the relay, disabling direct P2P. Requires at least one `--relay-url`. |
| `-h`, `--help` | Show help and exit. |

Examples:

```bash
# Default: public relay + iroh discovery server (needs internet), no override
./test-scripts/run_e2e.sh

# One custom relay -> discovery="none" path
./test-scripts/run_e2e.sh --relay-url https://relay.example.com

# Multiple relays (failover; uses public discovery)
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

Install the relay with `cargo install iroh-relay` if you don't have it. See
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

## Running the pieces by hand

The Python helpers are usable on their own:

```bash
uv run test-scripts/echo_server.py --proto tcp --port 9000
uv run test-scripts/echo_client.py --proto tcp --port 9000 --message hi
```
