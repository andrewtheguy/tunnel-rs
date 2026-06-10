# Performance Tuning

tunnel-rs runs over QUIC (UDP) via iroh. Out of the box it applies aggressive
transport tuning, but two OS-level limits — especially on Linux — can cap
throughput well below what the link supports.

## Linux: raise UDP socket buffer limits (important)

iroh requests 7 MB UDP socket buffers, but Linux silently clamps them to
`net.core.rmem_max` / `net.core.wmem_max`, which default to about 212 KB. The
result is packet loss under load and throughput that decays as the transfer
runs. Raise the limits on any Linux host running tunnel-rs (server or client):

```bash
sudo sysctl -w net.core.rmem_max=26214400 net.core.wmem_max=26214400
```

To persist across reboots:

```bash
sudo tee /etc/sysctl.d/99-tunnel-rs.conf <<EOF
net.core.rmem_max=26214400
net.core.wmem_max=26214400
EOF
sudo sysctl --system
```

To confirm the problem before/after, watch UDP receive buffer drops while a
transfer is running:

```bash
netstat -us | grep -i 'receive buffer errors'   # or: ss -unm
```

## macOS

No action needed: the default `kern.ipc.maxsockbuf` (8 MB) already accommodates
the 7 MB buffers iroh requests.

macOS has no UDP GSO/GRO, so each packet normally costs one syscall. tunnel-rs
works around this by patching the `netwatch` dependency (see `[patch.crates-io]`
in `Cargo.toml`) to enable Apple's batched `sendmsg_x`/`recvmsg_x` APIs — up to
32 packets per syscall. The patch can be dropped once upstream netwatch enables
this itself; if the APIs are unavailable on a given system, the code falls back
to standard `sendmsg`/`recvmsg` automatically.

## What tunnel-rs tunes built-in

- **QUIC ACK frequency extension**: peers ACK every ~10th packet instead of
  every 2nd, cutting ACK traffic ~5x.
- **MTU discovery up to 65527 bytes**: the path MTU is binary-searched instead
  of capping packets at 1452 bytes. Jumbo-frame LANs and loopback paths gain
  substantially; ordinary 1500-MTU paths settle at ~1452 as before.
- **BBR congestion control by default**: model-based and loss-tolerant, so
  throughput doesn't collapse when UDP buffers drop packets. Set
  `congestion_controller = "cubic"` under `[iroh.transport]` if you need strict
  loss-based fairness with competing flows.
- **16 MB receive / 32 MB send flow-control windows** (configurable via
  `receive_window` / `send_window` under `[iroh.transport]`).
- **4 MB TCP socket buffers, TCP_NODELAY** on the forwarded TCP connections.

## Benchmarking

Use iperf3 through the tunnel and compare both directions, with a long enough
run to see congestion behavior (not just the first seconds):

```bash
# on the server host
iperf3 -s

# tunnel the iperf3 port, then on the client host:
iperf3 -c 127.0.0.1 -p <local-tunnel-port> -t 30      # upload
iperf3 -c 127.0.0.1 -p <local-tunnel-port> -t 30 -R   # download
```

Check the tunnel log to confirm the connection is direct (not relayed) before
comparing numbers. If per-second throughput decays over the run, suspect UDP
buffer drops (see the Linux sysctls above).

Expectations: a userspace QUIC stack does per-packet crypto and processing that
kernel-accelerated TCP (e.g. an SSH tunnel) does not, so some gap to raw
TCP throughput is expected, particularly on macOS.
