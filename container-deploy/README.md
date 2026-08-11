# Container Deployment

Docker and Kubernetes configurations for running tunnel-rs in containerized environments.

> [!IMPORTANT]
> **Project Goal:** This tool provides a convenient way to connect to different networks for **development or homelab purposes**, conveniently forwarding both **TCP and UDP ports** without the hassle and security risk of opening a port on a public firewall. It is **not** meant for production setups or designed to be performant at scale.

> [!WARNING]
> **No Backward Compatibility (Pre-1.0):** During initial development before version 1.0, no backward compatibility or migration path is provided between minor versions (e.g., 0.1.x to 0.2.x). Expect to regenerate server keys and rebuild client/server configurations when upgrading between minor versions. To avoid unexpected breakage, pin the container image to a specific release tag (e.g., `ghcr.io/andrewtheguy/tunnel-rs:v0.4.0`) or minor version tag (e.g., `ghcr.io/andrewtheguy/tunnel-rs:0.4`).

## How It Works

tunnel-rs uses a **client-initiated** model similar to SSH `-L` tunneling:

| SSH Equivalent | tunnel-rs | Description |
|----------------|-----------|-------------|
| `ssh -L 8080:service:80` | Client with `--source` | Client requests what to tunnel |
| `sshd` with allowed hosts | Server with `--allowed-tcp` | Server whitelists allowed networks |

**Server** (runs in container, waits for connections):
- Uses `--allowed-tcp` / `--allowed-udp` with **CIDR notation** (e.g., `10.0.0.0/8`) to whitelist networks
- Uses `--authorized-keys-file` to authenticate Ed25519 client keys
- Does NOT specify ports — clients choose the destination

**Client** (initiates connection from remote machine):
- Uses `--source` with **protocol + address** (e.g., `tcp://postgres:5432` or `udp://kube-dns.kube-system.svc.cluster.local:53`) to request a specific service
- Uses `--target` to specify local listen address
- Uses `--private-key-file` to prove possession of an authorized Ed25519 key

## Quick Start

```bash
# 1. Generate server key
tunnel-rs generate-server-key --output server.key
# Output: EndpointId: <SERVER_NODE_ID>

# 2. Create a client authentication key and server entry
tunnel-rs generate-auth-key --output client.key --comment "remote client" \
  > authorized_keys

# 3. Server: allow connections from the generated public key
tunnel-rs server \
  --secret-file ./server.key \
  --allowed-tcp 127.0.0.0/8 \
  --allowed-tcp 192.168.0.0/16 \
  --authorized-keys-file ./authorized_keys
# Output: EndpointId: <SERVER_NODE_ID>

# 4. Client: connect and request a service
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://127.0.0.1:22 \
  --target 127.0.0.1:2222
```

## Docker

> [!NOTE]
> The Docker setup below has not been tested yet. Please report any issues.

Expose services via tunnel-rs with Ed25519 public-key authentication:

```bash
cd container-deploy/docker

# 1. Generate server key
docker run --rm ghcr.io/andrewtheguy/tunnel-rs:latest \
  generate-server-key --output - > server.key

# 2. Generate a client authentication key and server entry
docker run --rm -v "$PWD:/keys" ghcr.io/andrewtheguy/tunnel-rs:latest \
  generate-auth-key --output /keys/client.key --comment "remote client" \
  > authorized_keys

# 3. Start services
docker compose up -d

# 4. Get server EndpointId
docker compose logs tunnel-server | grep EndpointId
# EndpointId: <SERVER_NODE_ID>

# 5. On remote machine - connect to web service
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://web:80 \
  --target 127.0.0.1:8080

# 6. Or connect to database
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://db:5432 \
  --target 127.0.0.1:5432

# Access at http://127.0.0.1:8080 or localhost:5432
```

## Kubernetes

Access ClusterIP services from outside the cluster — like SSH tunneling but over P2P:

```bash
# 1. Generate server key
tunnel-rs generate-server-key --output server.key

# 2. Create a client authentication key and server entry
tunnel-rs generate-auth-key --output client.key --comment "remote client" \
  > authorized_keys

# 3. Create secrets
kubectl create secret generic tunnel-server-secrets \
  --from-file=server.key=./server.key \
  --from-file=authorized_keys=./authorized_keys

# 4. Deploy
kubectl apply -f kubernetes/tunnel-deployment.yaml

# 5. Get server EndpointId
kubectl logs -l app=tunnel-server | grep EndpointId
```

**Client examples** (run on your local machine):

```bash
# Tunnel to PostgreSQL
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://postgres.database.svc:5432 \
  --target 127.0.0.1:5432

# Tunnel to Redis
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://redis.cache.svc:6379 \
  --target 127.0.0.1:6379

# Tunnel to a web dashboard
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://kubernetes-dashboard.kubernetes-dashboard.svc:443 \
  --target 127.0.0.1:8443
```

**Advantages over `kubectl port-forward`:**
- Supports UDP (kubectl doesn't)
- Works across NAT without kubectl access
- QUIC keepalive and QUIC stream-open retry logic
- No need for cluster credentials on client
- Multiple clients can connect simultaneously

### UDP Example

Tunnel UDP services like DNS (something `kubectl port-forward` can't do):

```bash
# Expose cluster DNS
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source udp://kube-dns.kube-system.svc.cluster.local:53 \
  --target 127.0.0.1:5353

# Query cluster DNS locally
dig @127.0.0.1 -p 5353 kubernetes.default.svc.cluster.local
```

### Kubernetes Networking: hostNetwork

The default deployment uses `hostNetwork: true` so the pod shares the node's network namespace. This enables direct P2P connections via NAT hole-punching instead of falling back to relay servers.

- **`hostNetwork: true`** — pod uses the node's network stack directly, so STUN discovers the node's real external address
- **`dnsPolicy: ClusterFirstWithHostNet`** — required to resolve cluster DNS names (e.g., `service.namespace.svc.cluster.local`). Without this, the pod uses the node's `/etc/resolv.conf` and can't resolve K8s service names.

> [!NOTE]
> The pod can still access ClusterIP services and pod IPs — kube-proxy rules and CNI routes are installed at the node level, so hostNetwork pods inherit them.

With `hostNetwork: true`, traffic from the pod appears as node-originated, so Kubernetes network policies do not apply. If you need network policy enforcement (e.g., in multi-tenant clusters), remove `hostNetwork: true` and `dnsPolicy: ClusterFirstWithHostNet` from the deployment. Without `hostNetwork`, the pod uses K8s overlay networking where NAT hole-punching cannot work — connections will fall back to iroh relay servers. Consider using a [self-hosted relay](../docs/SELF-HOSTING.md) for lower latency in this case.

## Use Cases

| Scenario | Description |
|----------|-------------|
| Dev/staging access | Access services without exposing them publicly |
| Cluster-wide access | Single server, multiple services |
| UDP tunneling | DNS, WireGuard, game servers |
| NAT traversal | Works behind restrictive firewalls |
