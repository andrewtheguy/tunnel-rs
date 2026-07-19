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

Both tunnel-rs processes are configured via **JSON on stdin** (`--config-stdin`),
which exercises that both `server` and `client` accept stdin config. The Python
backends and test clients run through **`uv run`** (PEP 723 inline metadata, no
third-party dependencies).

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
  default iroh discovery server). Not needed when you point at your own relay
  (see `RELAY_URL` below).

## Usage

```bash
./test-scripts/run_e2e.sh
```

### Environment overrides

| Variable | Default | Meaning |
|----------|---------|---------|
| `TUNNEL_RS_BIN` | `target/debug/tunnel-rs` | Path to the tunnel-rs binary |
| `RELAY_URL` | _(unset)_ | Custom relay URL for both sides. When set, iroh discovery is **disabled automatically** and both sides rendezvous via this relay. Exercises the custom-relay code path. |
| `READY_TIMEOUT` | `60` | Seconds to wait for each process to become ready |
| `KEEP_LOGS` | `0` | Set to `1` to keep the temp working dir (configs + logs) for inspection |

Examples:

```bash
# Default: public relay + iroh discovery server (needs internet)
./test-scripts/run_e2e.sh

# Custom relay -> iroh discovery disabled path
RELAY_URL=https://relay.example.com ./test-scripts/run_e2e.sh

# Keep the generated JSON configs and per-process logs for debugging
KEEP_LOGS=1 ./test-scripts/run_e2e.sh
```

Exit code is `0` when both TCP and UDP round trips pass, non-zero otherwise.

## Running the pieces by hand

The Python helpers are usable on their own:

```bash
uv run test-scripts/echo_server.py --proto tcp --port 9000
uv run test-scripts/echo_client.py --proto tcp --port 9000 --message hi
```
