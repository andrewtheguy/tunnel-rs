# Container Deployment

Docker and Kubernetes configurations for running tunnel-rs in containerized
environments.

New to tunnel-rs? Read the [project README](../README.md) first — it covers the
project's scope, installation, key generation, CLI flags, and configuration
files. This page covers only what is specific to containers, and the manifests in
[`docker/`](docker/) and [`kubernetes/`](kubernetes/) are the runnable form of it.

> [!WARNING]
> **Pin the image tag.** No backward compatibility is provided before 1.0 (see
> the [project README](../README.md)), so a moving tag like `:latest` — or even
> the minor tag `:0.5` — can break a working deployment on the next release.
> Every example here, including the manifests, uses one immutable reference:
>
> ```
> ghcr.io/andrewtheguy/tunnel-rs:v0.5.0
> ```
>
> Change it in one place per deployment and keep client and server on the same
> release. Pinning by digest (`ghcr.io/andrewtheguy/tunnel-rs@sha256:…`, from
> `docker buildx imagetools inspect`) is stricter still, since a tag can be
> re-pushed.

## The deployment shape

tunnel-rs is **client-initiated**, like SSH `-L` tunneling — which decides what
goes in the container and what stays on your machine:

| SSH equivalent | tunnel-rs | Runs |
|---|---|---|
| `sshd` with allowed hosts | server, `--allowed-tcp` / `--allowed-udp` CIDRs | **in the cluster / compose network** |
| `ssh -L 8080:service:80` | client, `--source` / `--target` | **on your local machine** |

The consequence for containers: the server is deployed once and whitelists
*networks* rather than naming individual services — it publishes no ports of its
own, and each client picks a service inside those networks at connect time. One
deployed server serves every service in its allowed CIDRs, for any number of
simultaneous clients — you do not deploy one tunnel per service.

Both sides need keys before anything runs: a
[client authentication key](../README.md#client-authentication-key), and a
[server identity](../README.md#server-identity) that authorizes it and gives
clients a stable EndpointId to dial. The walkthroughs below generate both on the
operator's machine, so the order there does not matter; the generation commands
appear in context in each.

## Docker

> [!NOTE]
> The Docker setup below has not been tested yet. Please report any issues.

Exposes the `web` and `db` services from
[`docker/docker-compose.yml`](docker/docker-compose.yml), neither of which
publishes a port to the host:

```bash
cd container-deploy/docker

# 1. Generate the server key
docker run --rm ghcr.io/andrewtheguy/tunnel-rs:v0.5.0 \
  generate-server-key > server.key

# 2. Generate a client auth key on the host with the uv-run script
#    (key file to stdout, authorized-key entry to stderr)
../../scripts/generate-auth-key.py "remote client" \
  > client.key 2> authorized_keys
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
directly. The server's allowed CIDRs must cover that network — but only that
network. The example ships `172.16.0.0/12` and `192.168.0.0/16` so it starts on
any Docker installation; both are far wider than one compose project, and every
address inside an allowed CIDR is reachable by any authorized client. Narrow them
to the subnet actually in use:

```bash
docker network inspect docker_default \
  -f '{{range .IPAM.Config}}{{.Subnet}}{{end}}'   # e.g. 172.18.0.0/16
```

## Kubernetes

Reach ClusterIP services from outside the cluster, without cluster credentials on
the client:

```bash
# 1. Generate the server key
tunnel-rs generate-server-key --output server.key

# 2. Generate a client auth key with the uv-run script
#    (key file to stdout, authorized-key entry to stderr)
scripts/generate-auth-key.py "remote client" > client.key 2> authorized_keys
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

Then, from your local machine — fully qualified names (`svc.cluster.local`, or
whatever `--cluster-domain` your kubelets use), since `--source` is resolved by
the server pod and should not depend on its resolver search path:

```bash
# PostgreSQL
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://postgres.database.svc.cluster.local:5432 \
  --target 127.0.0.1:5432

# Redis
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://redis.cache.svc.cluster.local:6379 \
  --target 127.0.0.1:6379

# Cluster DNS over UDP — something kubectl port-forward cannot do
tunnel-rs client \
  --private-key-file ./client.key \
  --server-node-id <SERVER_NODE_ID> \
  --source udp://kube-dns.kube-system.svc.cluster.local:53 \
  --target 127.0.0.1:5353

dig @127.0.0.1 -p 5353 kubernetes.default.svc.cluster.local
```

> [!IMPORTANT]
> **Narrow the allowed CIDRs to your cluster's actual ranges.** The manifest
> ships `10.0.0.0/8` and `172.16.0.0/12` so it starts anywhere, but they are a
> high-trust opt-in: any authorized client can reach *every* address in an
> allowed CIDR through the server, not just the services you had in mind. Look up
> the real ranges and replace them:
>
> ```bash
> kubectl get nodes -o jsonpath='{.items[*].spec.podCIDR}'          # pod CIDR(s)
> kubectl -n kube-system get pod -l component=kube-apiserver \
>   -o jsonpath='{.items[0].spec.containers[0].command}' | tr ',' '\n' \
>   | grep service-cluster-ip-range                                 # service CIDR
> ```
>
> On managed clusters (EKS/GKE/AKS) the apiserver flags are not visible; take the
> service CIDR from the provider's cluster description instead, or infer it from
> `kubectl get svc kubernetes -o jsonpath='{.spec.clusterIP}'`.

**Compared with `kubectl port-forward`:** supports UDP, needs no cluster
credentials or `kubectl` on the client, works across NAT, survives as a
persistent deployment, adds QUIC keepalive and stream-open retry, and serves
multiple simultaneous clients from one deployment.

### hostNetwork and direct connections

The deployment sets `hostNetwork: true` with
`dnsPolicy: ClusterFirstWithHostNet`. Both matter:

- **`hostNetwork: true`** puts the pod in the *node's* network namespace rather
  than the pod's, so address discovery sees the node's addresses instead of the
  overlay's and hole punching has a chance to work. It improves the odds; it
  guarantees nothing. The node itself may sit behind a cloud NAT gateway or
  CGNAT, and an egress firewall blocks the UDP that hole punching needs either
  way — in which case the connection is carried by a relay, as it would have been
  without `hostNetwork`. Whether the overlay was the obstacle in the first place
  depends on your CNI and kube-proxy mode, not on Kubernetes as such — some
  clusters hole-punch from inside a normal pod just fine. See
  [Kubernetes and container networking](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/nat-traversal-and-transport.md#kubernetes-and-container-networking)
  before assuming you need this.
- **`dnsPolicy: ClusterFirstWithHostNet`** is required to keep resolving cluster
  DNS names such as `service.namespace.svc.cluster.local`. Without it a
  hostNetwork pod inherits the node's `/etc/resolv.conf` and cannot resolve
  service names — which breaks `--source`.

> [!NOTE]
> A hostNetwork pod still reaches ClusterIP services and pod IPs: kube-proxy
> rules and CNI routes are installed at the node level, so it inherits them.

**The trade-off is network policy.** With `hostNetwork: true` traffic appears
node-originated, and `NetworkPolicy` enforcement for host-network pods is
plugin-dependent — several CNIs do not apply pod policies to it at all. Check
your CNI's documentation rather than assuming it still applies.

If you need guaranteed policy enforcement — multi-tenant clusters especially —
drop both fields from the deployment. The pod then runs on the cluster's normal
pod networking, where hole punching may or may not succeed depending on the CNI;
if it doesn't, connections fall back to a relay. That works, but every byte takes
an extra hop, so consider a
[self-hosted relay](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/self-hosting.md)
near the cluster to keep the latency cost down.
