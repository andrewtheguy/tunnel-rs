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
permissions, and verifies that an unlisted key is rejected with the
authentication exit code. Authentication key files live only in the
automatically removed temporary working directory.

This suite covers what tunnel-rs adds on top of the shared iroh layer: the
tunnel itself. The shared layer — the auth transcript over iroh, relay
connectivity, the startup probe, and the in-place home-relay failover — is
tested end to end in
[flexaccess-iroh](https://github.com/flexaccessdev/flexaccess-iroh/tree/main/e2e),
against a minimal harness instead of tunnel-rs. The Python backends and
test clients run through **`uv run`** (PEP 723 inline metadata, no third-party
dependencies).

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
| `--relay-url URL` | Custom relay URL for both sides (**repeatable**; give at least two, a single custom relay is rejected). Custom relays disable internet discovery automatically; clients reach the server through relay hints. Also accepts `--relay-url=URL`. |
| `--relay-only` | Force all traffic through the relays, disabling direct P2P. Requires `--relay-url`. |
| `-h`, `--help` | Show help and exit. |

Examples:

```bash
# Default: public relay + iroh discovery server (needs internet), no override
./test-scripts/run_e2e.sh

# Custom relays (at least two; internet discovery auto-disabled)
./test-scripts/run_e2e.sh --relay-url https://r1.example.com --relay-url https://r2.example.com

# Relay-only e2e (no direct P2P; requires custom relays)
./test-scripts/run_e2e.sh --relay-url https://r1.example.com --relay-url https://r2.example.com --relay-only
```

Exit code is `0` when both TCP and UDP round trips pass, non-zero otherwise.

### Running local relays for offline relay-only tests

`--relay-only` needs reachable custom relays, and a custom relay set is at
least two relays. To run fully offline, start two local `iroh-relay` instances
in dev mode using the bundled configs (`relay-dev.toml` on port 3340 and
`relay-dev-2.toml` on port 3341; both disable the metrics server so it won't
collide with port 9090):

```bash
# terminal 1: local dev relay on http://localhost:3340
iroh-relay --dev -c test-scripts/relay-dev.toml

# terminal 2: second local dev relay on http://localhost:3341
iroh-relay --dev -c test-scripts/relay-dev-2.toml

# terminal 3: relay-only e2e against them
./test-scripts/run_e2e.sh --relay-url http://localhost:3340 --relay-url http://localhost:3341 --relay-only
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
| `RELAY_URL` | _(unset)_ | Fallback custom relays, whitespace-separated, used **only** when no `--relay-url` flag is given (prefer the flag) |

```bash
# Keep per-process logs for debugging
KEEP_LOGS=1 ./test-scripts/run_e2e.sh
```

## Relay failover test

The relay failover scenarios (a relay down before startup, a relay dying after
startup, a home relay that answers probes but refuses connections) exercise the
shared layer only, so they live with it:
[`e2e/run_relay_failover.sh` in flexaccess-iroh](https://github.com/flexaccessdev/flexaccess-iroh/tree/main/e2e).
tunnel-rs picks the behavior up by bumping its `flexaccess-iroh` tag.

## Running the pieces by hand

The Python helpers are usable on their own:

```bash
uv run test-scripts/echo_server.py --proto tcp --port 9000
uv run test-scripts/echo_client.py --proto tcp --port 9000 --message hi
```
