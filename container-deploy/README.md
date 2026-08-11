# Container Deployment

Docker and Kubernetes configurations for running tunnel-rs in containerized
environments.

New to tunnel-rs? Read the [project README](../README.md) first — it covers the
project's scope, installation, key generation, CLI flags, and configuration
files. This page covers only what is specific to containers, and the manifests in
[`docker/`](docker/) and [`kubernetes/`](kubernetes/) are the runnable form of it.

> [!WARNING]
> **Pin the image tag.** No backward compatibility is provided before 1.0 (see
> the [project README](../README.md)), so `:latest` can break a working
> deployment on the next release. Pin to a release tag
> (`ghcr.io/andrewtheguy/tunnel-rs:v0.5.0`) or a minor version tag
> (`ghcr.io/andrewtheguy/tunnel-rs:0.5`). The examples here use `:latest` for
> brevity.

## The deployment shape

tunnel-rs is **client-initiated**, like SSH `-L` tunneling — which decides what
goes in the container and what stays on your machine:

| SSH equivalent | tunnel-rs | Runs |
|---|---|---|
| `sshd` with allowed hosts | server, `--allowed-tcp` / `--allowed-udp` CIDRs | **in the cluster / compose network** |
| `ssh -L 8080:service:80` | client, `--source` / `--target` | **on your local machine** |

The consequence for containers: the server is deployed once, names no ports, and
whitelists *networks*; each client then picks a service inside those networks at
connect time. One deployed server serves every service in its allowed CIDRs, for
any number of simultaneous clients — you do not deploy one tunnel per service.

Both sides need keys before anything runs: a server identity (stable EndpointId)
and at least one authorized client key. See
[Persistent Server Identity](../README.md#persistent-server-identity) and
[Authentication](../README.md#authentication); the generation commands appear in
context in the two walkthroughs below.

## Docker

> [!NOTE]
> The Docker setup below has not been tested yet. Please report any issues.

Exposes the `web` and `db` services from
[`docker/docker-compose.yml`](docker/docker-compose.yml), neither of which
publishes a port to the host:

```bash
cd container-deploy/docker

# 1. Generate the server key
docker run --rm ghcr.io/andrewtheguy/tunnel-rs:latest \
  generate-server-key --output - > server.key

# 2. Generate a client auth key; the authorized-key entry goes to the server
docker run --rm -v "$PWD:/keys" ghcr.io/andrewtheguy/tunnel-rs:latest \
  generate-auth-key --output /keys/client.key --comment "remote client" \
  > authorized_keys
# Copy client.key securely to the client machine.

# 3. Start services
docker compose up -d

# 4. Read the server EndpointId
docker compose logs tunnel-server | grep EndpointId
# EndpointId: <SERVER_NODE_ID>
```

Then, from the remote machine:

```bash
# Web service
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://web:80 \
  --target 127.0.0.1:8080

# Database
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://db:5432 \
  --target 127.0.0.1:5432

# Reachable at http://127.0.0.1:8080 and localhost:5432
```

`--source` resolves inside the compose network, so Docker service names work
directly. The server's allowed CIDRs must cover the compose network
(`172.16.0.0/12` and `192.168.0.0/16` in the example).

## Kubernetes

Reach ClusterIP services from outside the cluster, without cluster credentials on
the client:

```bash
# 1. Generate the server key
tunnel-rs generate-server-key --output server.key

# 2. Generate a client auth key; the authorized-key entry goes to the server
tunnel-rs generate-auth-key --output client.key --comment "remote client" \
  > authorized_keys
# Copy client.key securely to the client machine.

# 3. Create the secret the deployment mounts
kubectl create secret generic tunnel-server-secrets \
  --from-file=server.key=./server.key \
  --from-file=authorized_keys=./authorized_keys

# 4. Deploy
kubectl apply -f kubernetes/tunnel-deployment.yaml

# 5. Read the server EndpointId
kubectl logs -l app=tunnel-server | grep EndpointId
```

Then, from your local machine — full DNS names, since `--source` is resolved by
the server pod:

```bash
# PostgreSQL
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://postgres.database.svc:5432 \
  --target 127.0.0.1:5432

# Redis
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://redis.cache.svc:6379 \
  --target 127.0.0.1:6379

# Cluster DNS over UDP — something kubectl port-forward cannot do
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source udp://kube-dns.kube-system.svc.cluster.local:53 \
  --target 127.0.0.1:5353

dig @127.0.0.1 -p 5353 kubernetes.default.svc.cluster.local
```

**Compared with `kubectl port-forward`:** supports UDP, needs no cluster
credentials or `kubectl` on the client, works across NAT, survives as a
persistent deployment, adds QUIC keepalive and stream-open retry, and serves
multiple simultaneous clients from one deployment.

### hostNetwork and direct connections

The deployment sets `hostNetwork: true` with
`dnsPolicy: ClusterFirstWithHostNet`. Both matter:

- **`hostNetwork: true`** puts the pod in the *node's* network namespace rather
  than the pod's, so address discovery sees the node's real external address and
  hole punching can work. Kubernetes overlay networking is the classic case where
  it usually cannot — see
  [symmetric NAT and container overlays](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/nat-traversal-and-transport.md#symmetric-nat-and-container-overlays)
  for why.
- **`dnsPolicy: ClusterFirstWithHostNet`** is required to keep resolving cluster
  DNS names such as `service.namespace.svc.cluster.local`. Without it a
  hostNetwork pod inherits the node's `/etc/resolv.conf` and cannot resolve
  service names — which breaks `--source`.

> [!NOTE]
> A hostNetwork pod still reaches ClusterIP services and pod IPs: kube-proxy
> rules and CNI routes are installed at the node level, so it inherits them.

**The trade-off is network policy.** With `hostNetwork: true`, traffic appears
node-originated and Kubernetes network policies do not apply to it. If you need
policy enforcement — multi-tenant clusters especially — drop both fields from the
deployment. The pod then runs on overlay networking, where hole punching usually
fails and connections fall back to a relay. That works, but every byte takes an
extra hop, so consider a
[self-hosted relay](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/self-hosting.md)
near the cluster to keep the latency cost down.
