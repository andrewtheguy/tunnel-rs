# Container Deployment

Docker and Kubernetes configurations for running tunnel-rs in containerized environments.

Note: The container image `ghcr.io/andrewtheguy/tunnel-rs:latest` is iroh-only.
The `tunnel-rs-ice` binary is published in GitHub releases but is not containerized.

> [!TIP]
> **Recommended Mode:** Use iroh mode for all deployments. It is the default behavior for `tunnel-rs server` and `tunnel-rs client`, and provides the best NAT traversal with relay fallback, client authentication via tokens, and multi-source capability where clients choose what to tunnel.

> [!WARNING]
> **No Backward Compatibility (Pre-1.0):** During initial development before version 1.0, no backward compatibility or migration path is provided between minor versions (e.g., 0.1.x to 0.2.x). Expect to regenerate server keys and rebuild client/server configurations when upgrading.

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
- Uses `--source` with **hostname:port** (e.g., `tcp://postgres:5432`) to request a specific service
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

# 3. Server: allow connections with token authentication
tunnel-rs server \
  --secret-file ./server.key \
  --allowed-tcp 127.0.0.0/8 \
  --allowed-tcp 192.168.0.0/16 \
  --auth-tokens "$AUTH_TOKEN"
# Output: EndpointId: <SERVER_NODE_ID>

# 4. Client: connect and request a service
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://127.0.0.1:22 \
  --target 127.0.0.1:2222 \
  --auth-token "$AUTH_TOKEN"
```

## Docker

Expose services via tunnel-rs with token authentication:

```bash
cd docker

# 1. Generate server key
docker run --rm ghcr.io/andrewtheguy/tunnel-rs:latest \
  generate-server-key --output - > server.key

# 2. Create an authentication token
AUTH_TOKEN=$(docker run --rm ghcr.io/andrewtheguy/tunnel-rs:latest generate-token)
echo "$AUTH_TOKEN" > tokens.txt

# 3. Start services (update docker-compose.yml to mount tokens.txt)
docker compose up -d

# 4. Get server EndpointId
docker compose logs tunnel-server | grep EndpointId
# EndpointId: <SERVER_NODE_ID>

# 5. On remote machine - connect to web service
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://web:80 \
  --target 127.0.0.1:8080 \
  --auth-token "$AUTH_TOKEN"

# 6. Or connect to database
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://db:5432 \
  --target 127.0.0.1:5432 \
  --auth-token "$AUTH_TOKEN"

# Access at http://127.0.0.1:8080 or localhost:5432
```

## Kubernetes

Access ClusterIP services from outside the cluster — like SSH tunneling but over P2P:

```bash
# 1. Generate server key
tunnel-rs generate-server-key --output server.key

# 2. Create an authentication token
AUTH_TOKEN=$(tunnel-rs generate-token)

# 3. Create secrets
kubectl create secret generic tunnel-server-secrets \
  --from-file=server.key=./server.key \
  --from-literal=tokens.txt="$AUTH_TOKEN"

# 4. Deploy
kubectl apply -f kubernetes/tunnel-deployment.yaml

# 5. Get server EndpointId
kubectl logs -l app=tunnel-server | grep EndpointId
```

**Client examples** (run on your local machine):

```bash
# Tunnel to PostgreSQL
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://postgres.database.svc:5432 \
  --target 127.0.0.1:5432 \
  --auth-token "$AUTH_TOKEN"

# Tunnel to Redis
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://redis.cache.svc:6379 \
  --target 127.0.0.1:6379 \
  --auth-token "$AUTH_TOKEN"

# Tunnel to a web dashboard
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://kubernetes-dashboard.kubernetes-dashboard.svc:443 \
  --target 127.0.0.1:8443 \
  --auth-token "$AUTH_TOKEN"
```

**Advantages over `kubectl port-forward`:**
- Supports UDP (kubectl doesn't)
- Works across NAT without kubectl access
- Persistent connections with auto-reconnect
- No need for cluster credentials on client
- Multiple clients can connect simultaneously

### UDP Example

Tunnel UDP services like DNS (something `kubectl port-forward` can't do):

```bash
# Expose cluster DNS
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source udp://kube-dns.kube-system.svc.cluster.local:53 \
  --target udp://127.0.0.1:5353 \
  --auth-token "$AUTH_TOKEN"

# Query cluster DNS locally
dig @127.0.0.1 -p 5353 kubernetes.default.svc.cluster.local
```

## Use Cases

| Scenario | Description |
|----------|-------------|
| Dev/staging access | Access services without exposing them publicly |
| Cluster-wide access | Single server, multiple services |
| UDP tunneling | DNS, WireGuard, game servers |
| NAT traversal | Works behind restrictive firewalls |
