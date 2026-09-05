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

`run_relay_failover_e2e.sh` is a separate, fully offline suite that starts
**two local `iroh-relay --dev` instances** and exercises relay failures in
relay-only mode. Servers and clients are each given an explicit relay list per
scenario.

The contract under test: a custom relay set holds **at least two relays**, a
server rides out a relay outage by moving onto another configured relay **in
place** (same process, same endpoint, connections untouched), and startup fails
only when **no** configured relay is reachable (a dead relay is a warning).

- **Phase A (relay down before startup):** a server configured with both relays
  fails to start when both are down and starts with a warning when one is; a
  client configured with both connects through the live one; a single custom
  relay is rejected as configuration.
- **Phase B (relay dies after startup):** the server's home relay is killed —
  the running server stays up and re-homes onto the survivor on its own
  (~30s); a restarted client configured with both relays reconnects; with both
  relays killed new clients fail; after both relays restart, clients connect
  again.
- **Phase C (home relay answers probes but refuses connections):** the case
  iroh does not recover from on its own. relay1 is replaced on its port by
  `fake_relay.py` (200 on `/ping`, 404 on everything else) while relay2 sits
  behind `delay_proxy.py`, which adds latency so net_report keeps preferring
  the fake. After 60s the shared failover takes the fake out of the relay map,
  the server homes on relay2 without restarting anything, and a new client
  connects; when the real relay1 returns, the restore probe (every 90s) puts
  it back and the server moves back onto it. See
  [relay-failover.md](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/relay-failover.md).

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
