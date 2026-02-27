# Container Deployment

Docker and Kubernetes configurations for running tunnel-rs in containerized environments.

Note: The container image `ghcr.io/andrewtheguy/tunnel-rs:latest` is iroh-only.
The `tunnel-rs-ice` binary is published in GitHub releases but is not containerized.

> [!IMPORTANT]
> **Project Goal:** This tool provides a convenient way to connect to different networks for **development or homelab purposes** without the hassle and security risk of opening a port. It is **not** meant for production setups or designed to be performant at scale.

> [!WARNING]
> **No Backward Compatibility (Pre-1.0):** During initial development before version 1.0, no backward compatibility or migration path is provided between minor versions (e.g., 0.1.x to 0.2.x). Expect to regenerate server keys and rebuild client/server configurations when upgrading between minor versions. To avoid unexpected breakage, pin the container image to a specific patch version (e.g., `ghcr.io/andrewtheguy/tunnel-rs:0.2.0`) or minor version (e.g., `ghcr.io/andrewtheguy/tunnel-rs:0.2`).

> [!WARNING]
> **Breaking Changes (v0.2):**
> - **Auth token format:** Changed from 18-char Luhn mod N tokens to 47-char Base64URL tokens with CRC16 checksum. Old tokens are **not** accepted. Regenerate all tokens with `tunnel-rs generate-token` and update your `tokens.txt` files and client configurations.
> - **Silent auth failure:** The server no longer sends a rejection message or closes the connection with an error code — it simply waits out the auth timeout and drops the connection. Clients with invalid tokens will see a generic connection timeout instead of a structured rejection. Server-side logs still show the reason for operators.
> - **ALPN token required:** A new `--alpn-token` argument is required for both server and client. Generate with `tunnel-rs generate-token --alpn`.

## How It Works

tunnel-rs uses a **client-initiated** model similar to SSH `-L` tunneling:

| SSH Equivalent | tunnel-rs | Description |
|----------------|-----------|-------------|
| `ssh -L 8080:service:80` | Client with `--source` | Client requests what to tunnel |
| `sshd` with allowed hosts | Server with `--allowed-tcp` | Server whitelists allowed networks |

**Server** (runs in container, waits for connections):
- Uses `--allowed-tcp` / `--allowed-udp` with **CIDR notation** (e.g., `10.0.0.0/8`) to whitelist networks
- Uses `--auth-tokens` or `--auth-tokens-file` to authenticate clients by pre-shared token
- Does NOT specify ports — clients choose the destination

**Client** (initiates connection from remote machine):
- Uses `--source` with **protocol + address** (e.g., `tcp://postgres:5432` or `udp://kube-dns.kube-system.svc.cluster.local:53`) to request a specific service
- Uses `--target` to specify local listen address
- Uses `--auth-token` to authenticate with the server

## Quick Start

```bash
# 1. Generate server key
tunnel-rs generate-server-key --output server.key
# Output: EndpointId: <SERVER_NODE_ID>

# 2. Create an authentication token
AUTH_TOKEN=$(tunnel-rs generate-token)
echo $AUTH_TOKEN  # Share this with authorized clients

# 3. Create an ALPN token (shared between server and all clients)
ALPN_TOKEN=$(tunnel-rs generate-token --alpn)
echo $ALPN_TOKEN

# 4. Server: allow connections with token authentication
tunnel-rs server \
  --secret-file ./server.key \
  --allowed-tcp 127.0.0.0/8 \
  --allowed-tcp 192.168.0.0/16 \
  --auth-tokens "$AUTH_TOKEN" \
  --alpn-token "$ALPN_TOKEN"
# Output: EndpointId: <SERVER_NODE_ID>

# 5. Client: connect and request a service
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://127.0.0.1:22 \
  --target 127.0.0.1:2222 \
  --auth-token "$AUTH_TOKEN" \
  --alpn-token "$ALPN_TOKEN"
```

## Docker

Expose services via tunnel-rs with token authentication:

```bash
cd container-deploy/docker

# 1. Generate server key
docker run --rm ghcr.io/andrewtheguy/tunnel-rs:latest \
  generate-server-key --output - > server.key

# 2. Create an authentication token
AUTH_TOKEN=$(docker run --rm ghcr.io/andrewtheguy/tunnel-rs:latest generate-token)
echo "$AUTH_TOKEN" > tokens.txt

# 3. Create an ALPN token (shared between server and all clients)
docker run --rm ghcr.io/andrewtheguy/tunnel-rs:latest generate-token --alpn > alpn_token.txt
ALPN_TOKEN=$(cat alpn_token.txt)

# 4. Start services
docker compose up -d

# 5. Get server EndpointId
docker compose logs tunnel-server | grep EndpointId
# EndpointId: <SERVER_NODE_ID>

# 6. On remote machine - connect to web service
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://web:80 \
  --target 127.0.0.1:8080 \
  --auth-token "$AUTH_TOKEN" \
  --alpn-token "$ALPN_TOKEN"

# 7. Or connect to database
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://db:5432 \
  --target 127.0.0.1:5432 \
  --auth-token "$AUTH_TOKEN" \
  --alpn-token "$ALPN_TOKEN"

# Access at http://127.0.0.1:8080 or localhost:5432
```

## Kubernetes

Access ClusterIP services from outside the cluster — like SSH tunneling but over P2P:

```bash
# 1. Generate server key
tunnel-rs generate-server-key --output server.key

# 2. Create an authentication token
AUTH_TOKEN=$(tunnel-rs generate-token)

# 3. Create an ALPN token (shared between server and all clients)
tunnel-rs generate-token --alpn > alpn_token.txt
ALPN_TOKEN=$(cat alpn_token.txt)

# 4. Create secrets
kubectl create secret generic tunnel-server-secrets \
  --from-file=server.key=./server.key \
  --from-literal=tokens.txt="$AUTH_TOKEN" \
  --from-file=alpn-token=./alpn_token.txt

# 5. Deploy
kubectl apply -f kubernetes/tunnel-deployment.yaml

# 6. Get server EndpointId
kubectl logs -l app=tunnel-server | grep EndpointId
```

**Client examples** (run on your local machine):

```bash
# Tunnel to PostgreSQL
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://postgres.database.svc:5432 \
  --target 127.0.0.1:5432 \
  --auth-token "$AUTH_TOKEN" \
  --alpn-token "$ALPN_TOKEN"

# Tunnel to Redis
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://redis.cache.svc:6379 \
  --target 127.0.0.1:6379 \
  --auth-token "$AUTH_TOKEN" \
  --alpn-token "$ALPN_TOKEN"

# Tunnel to a web dashboard
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://kubernetes-dashboard.kubernetes-dashboard.svc:443 \
  --target 127.0.0.1:8443 \
  --auth-token "$AUTH_TOKEN" \
  --alpn-token "$ALPN_TOKEN"
```

**Advantages over `kubectl port-forward`:**
- Supports UDP (kubectl doesn't)
- Works across NAT without kubectl access
- QUIC keepalive and stream retry logic
- No need for cluster credentials on client
- Multiple clients can connect simultaneously

### UDP Example

Tunnel UDP services like DNS (something `kubectl port-forward` can't do):

```bash
# Expose cluster DNS
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source udp://kube-dns.kube-system.svc.cluster.local:53 \
  --target 127.0.0.1:5353 \
  --auth-token "$AUTH_TOKEN" \
  --alpn-token "$ALPN_TOKEN"

# Query cluster DNS locally
dig @127.0.0.1 -p 5353 kubernetes.default.svc.cluster.local
```

### Kubernetes Networking Options

By default, tunnel-rs pods use Kubernetes overlay networking. Because K8s overlay NAT is symmetric (conntrack-based), iroh's STUN-based hole-punching cannot establish direct P2P connections — all traffic falls back to iroh relay servers. This works out of the box but adds a relay hop.

#### hostNetwork for Direct P2P

To enable direct P2P connections and NAT hole-punching, run the pod with `hostNetwork: true`. This bypasses the overlay network entirely — the pod shares the node's network namespace, so STUN discovers the node's real external address.

Use the `tunnel-deployment-hostnetwork.yaml` variant:

```bash
kubectl apply -f kubernetes/tunnel-deployment-hostnetwork.yaml
```

Key differences from the standard deployment:

```yaml
spec:
  template:
    spec:
      hostNetwork: true
      dnsPolicy: ClusterFirstWithHostNet
```

- **`hostNetwork: true`** — pod uses the node's network stack directly
- **`dnsPolicy: ClusterFirstWithHostNet`** — required to resolve cluster DNS names (e.g., `service.namespace.svc.cluster.local`). Without this, the pod uses the node's `/etc/resolv.conf` and can't resolve K8s service names.

> [!NOTE]
> The pod can still access ClusterIP services and pod IPs — kube-proxy rules and CNI routes are installed at the node level, so hostNetwork pods inherit them.

**Tradeoffs:**
- No Kubernetes network policy enforcement (traffic appears as node-originated)
- Pod is exposed to all node network traffic

**Recommendation:** Use `hostNetwork: true` for single-node dev/homelab clusters where direct P2P is preferred. For multi-tenant or production clusters, use the default overlay deployment with a [self-hosted relay](../docs/SELF-HOSTING.md) for lower latency.

## Use Cases

| Scenario | Description |
|----------|-------------|
| Dev/staging access | Access services without exposing them publicly |
| Cluster-wide access | Single server, multiple services |
| UDP tunneling | DNS, WireGuard, game servers |
| NAT traversal | Works behind restrictive firewalls |
