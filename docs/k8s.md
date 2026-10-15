# Running `live-mutex-rs` on Kubernetes

This doc walks through deploying the regular Rust broker
(`oresoftware/live-mutex-rs`) as a cluster-internal locking service,
then shows the separate BrokerRaft HA shape. The regular Broker recipe
below is what we run in production: one replica, a ClusterIP `Service`
exposing both the TCP wire protocol and the HTTP/Prometheus front-end,
an `LMX_AUTH_TOKEN` sourced from a Kubernetes `Secret`, and a `Recreate`
strategy because all regular Broker lock state lives in process memory.

If you've never run a networked mutex broker before, please read
[the readme](../readme.md) first — especially the
"Known limitations" section, which explains why this is a single-
replica service by design.

## Topology at a glance

```
+----------------------+         TCP :6970 (newline-JSON wire)
|                      |  <----- HTTP :6971 (status, /v1/*, /metrics)
|  live-mutex-rs pod   |
|  - dd-rust-network-  |
|    mutex binary      |
|  - in-process locks  |
|                      |
+----------+-----------+
           ^
           | ClusterIP
           |
+----------+-----------+
|  Service             |
|  dd-rust-network-    |
|  mutex.<ns>:6970     |
|  dd-rust-network-    |
|  mutex.<ns>:6971     |
+----------+-----------+
           ^
           | (cluster-internal callers)
           |
   [your service A]   [your service B]   [Lambda → /v1/lock]
```

Single replica is the supported posture for the regular Broker. See
[Regular Broker: why single-replica](#regular-broker-why-single-replica)
below for the rationale. For replicated quorum locking, deploy
BrokerRaft as a separate workload rather than adding replicas to
`live-mutex-rs`.

## Container image

The broker is published on Docker Hub:

- [`oresoftware/live-mutex-rs:0.1.123`](https://hub.docker.com/r/oresoftware/live-mutex-rs)
- [`oresoftware/live-mutex-rs:latest`](https://hub.docker.com/r/oresoftware/live-mutex-rs)
  (rolls forward; pin to a specific tag in production)

The `Dockerfile` at the root of this repo is a multi-stage build
(`rust:1.90-bookworm` → `debian:bookworm-slim`), runs as
`uid:gid 65532:65532`, and exposes the two listener ports. Tags
follow the `Cargo.toml` version. To build your own image:

```bash
docker build -t my-registry/live-mutex-rs:dev .
docker push my-registry/live-mutex-rs:dev
```

Override defaults at build time with:

```bash
# TLS-only (no OTel exporter), smaller image:
docker build \
  --build-arg CARGO_BUILD_FLAGS="--no-default-features --features tls" \
  -t my-registry/live-mutex-rs:tls-only .

# Plain (no TLS, no OTel), smallest image:
docker build \
  --build-arg CARGO_BUILD_FLAGS="--no-default-features" \
  -t my-registry/live-mutex-rs:plain .
```

## Deployment manifest

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: live-mutex-rs
  namespace: default
  labels:
    app: live-mutex-rs
spec:
  # Single replica. All lock state lives in process memory; running
  # two pods would split your namespace into two independent locking
  # universes (which is almost certainly not what you want).
  replicas: 1
  strategy:
    # Recreate, not RollingUpdate. A rolling update would spin up a
    # second pod that holds different lock state from the old one,
    # which violates mutual exclusion during the rollover window.
    type: Recreate
  selector:
    matchLabels:
      app: live-mutex-rs
  template:
    metadata:
      labels:
        app: live-mutex-rs
    spec:
      automountServiceAccountToken: false
      terminationGracePeriodSeconds: 20
      containers:
        - name: live-mutex-rs
          image: docker.io/oresoftware/live-mutex-rs:0.1.123
          imagePullPolicy: IfNotPresent
          securityContext:
            allowPrivilegeEscalation: false
            runAsNonRoot: true
            seccompProfile:
              type: RuntimeDefault
            capabilities:
              drop:
                - ALL
          env:
            - name: LMX_BIND_HOST
              value: 0.0.0.0
            - name: LMX_TCP_PORT
              value: '6970'
            - name: LMX_HTTP_PORT
              value: '6971'
            # Default-on so the broker GC keeps the per-key fencing-
            # token counter monotonic across re-incarnations of a key.
            - name: LMX_DEFAULT_TTL_MS
              value: '4000'
            - name: LMX_MAX_LOCK_HOLDERS
              value: '1'
            - name: LMX_LOG_FORMAT
              value: text
            - name: RUST_LOG
              value: info,lmx=info
            # Required when the broker is exposed beyond its own
            # NetworkPolicy. Sourced from a Kubernetes Secret so it
            # can rotate without a deployment edit. Mark optional if
            # you want the manifest to roll out before the Secret is
            # populated.
            - name: LMX_AUTH_TOKEN
              valueFrom:
                secretKeyRef:
                  name: live-mutex-rs-secrets
                  key: LMX_AUTH_TOKEN
                  optional: true
          ports:
            - name: lmx-tcp
              containerPort: 6970
            - name: http
              containerPort: 6971
          resources:
            requests:
              cpu: 50m
              memory: 96Mi
            limits:
              cpu: '1'
              memory: 1Gi
          # During pod boot the broker's HTTP listener takes a few
          # hundred ms to come up. We use a TCP probe for `lmx-tcp`
          # as the startup signal and HTTP probes against `/healthz`
          # for readiness/liveness once the front-end is live.
          startupProbe:
            httpGet:
              path: /healthz
              port: http
            periodSeconds: 5
            failureThreshold: 60
          readinessProbe:
            httpGet:
              path: /healthz
              port: http
            periodSeconds: 5
            timeoutSeconds: 3
            failureThreshold: 2
          livenessProbe:
            httpGet:
              path: /healthz
              port: http
            periodSeconds: 30
            timeoutSeconds: 5
            failureThreshold: 3
```

## Service manifest

```yaml
apiVersion: v1
kind: Service
metadata:
  name: live-mutex-rs
  namespace: default
  labels:
    app: live-mutex-rs
  annotations:
    # Standard Prometheus scrape annotations. The HTTP listener
    # exposes /metrics in plain Prometheus exposition format under
    # the `dd_rust_network_mutex_*` namespace.
    prometheus.io/scrape: 'true'
    prometheus.io/port: '6971'
    prometheus.io/path: /metrics
spec:
  type: ClusterIP
  selector:
    app: live-mutex-rs
  ports:
    - name: lmx-tcp
      port: 6970
      targetPort: lmx-tcp
    - name: http
      port: 6971
      targetPort: http
```

Cluster-internal callers reach the broker at:

- `tcp://live-mutex-rs.default.svc.cluster.local:6970` for the wire
  protocol (use `Client` / `RwClient` from this crate, or any of the
  cross-runtime clients under `clients/`).
- `http://live-mutex-rs.default.svc.cluster.local:6971/v1/*` for
  serverless-style callers (Lambda, Workers) that can't hold a
  long-lived TCP connection.

## BrokerRaft HA deployment

BrokerRaft should be deployed as its own StatefulSet and Services, with
names that do not overwrite the regular `live-mutex-rs` Deployment. A
3-node cluster commits with 2 votes; a 5-node cluster commits with 3
votes. Public BrokerRaft admission supports single-key requests and bounded
multi-key `keys` requests up to 3 distinct keys, preserving the regular
Broker's union-overlap composite semantics while keeping replicated
per-log-index fencing-token reservation bounded.

Use one headless peer Service for stable Raft RPC DNS and one client
Service for HTTP traffic. The peer Service publishes not-ready
addresses so the Raft cluster can form even while no leader has passed
`/raft/leaderz` yet:

```yaml
apiVersion: v1
kind: Service
metadata:
  name: live-mutex-rs-raft-peers
  namespace: default
  labels:
    app: live-mutex-rs-raft
spec:
  clusterIP: None
  publishNotReadyAddresses: true
  selector:
    app: live-mutex-rs-raft
  ports:
    - name: raft
      port: 7980
      targetPort: raft
---
apiVersion: v1
kind: Service
metadata:
  name: live-mutex-rs-raft
  namespace: default
  labels:
    app: live-mutex-rs-raft
  annotations:
    prometheus.io/scrape: 'true'
    prometheus.io/port: '6971'
    prometheus.io/path: /metrics
spec:
  type: ClusterIP
  selector:
    app: live-mutex-rs-raft
  ports:
    - name: http
      port: 6971
      targetPort: http
```

Mount a Raft config whose peer IDs match the StatefulSet pod names.
This example is for the `default` namespace:

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: live-mutex-rs-raft-config
  namespace: default
data:
  lmx.toml: |
    [server]
    bind_host = "0.0.0.0"
    http_port = 6971
    disable_tcp = true
    disable_http = false

    [broker]
    default_ttl_ms = 4000

    [raft]
    enabled = true
    bind_addr = "0.0.0.0:7980"
    data_dir = "/var/lib/dd-rust-network-mutex/raft"
    data_dir_lock = true
    max_inbound_rpc_connections = 1024
    inbound_rpc_idle_timeout_ms = 30000
    heartbeat_interval_ms = 50
    election_timeout_min_ms = 150
    election_timeout_max_ms = 300
    snapshot_interval_ms = 1800000
    trailing_log_entries = 10000
    append_entries_max_entries = 256
    append_entries_max_bytes = 1048576
    append_entries_max_inline_batches = 64
    install_snapshot_chunk_bytes = 1048576
    sync_log = true

    [[raft.peers]]
    id = "live-mutex-rs-raft-0"
    addr = "live-mutex-rs-raft-0.live-mutex-rs-raft-peers.default.svc.cluster.local:7980"

    [[raft.peers]]
    id = "live-mutex-rs-raft-1"
    addr = "live-mutex-rs-raft-1.live-mutex-rs-raft-peers.default.svc.cluster.local:7980"

    [[raft.peers]]
    id = "live-mutex-rs-raft-2"
    addr = "live-mutex-rs-raft-2.live-mutex-rs-raft-peers.default.svc.cluster.local:7980"
```

Then run the Raft image as a StatefulSet. `podManagementPolicy: Parallel`
matters when the client Service uses leader-only readiness: with the
default ordered policy, pod 0 can wait for peers that Kubernetes has not
created yet.

```yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: live-mutex-rs-raft
  namespace: default
  labels:
    app: live-mutex-rs-raft
spec:
  serviceName: live-mutex-rs-raft-peers
  replicas: 3
  podManagementPolicy: Parallel
  selector:
    matchLabels:
      app: live-mutex-rs-raft
  template:
    metadata:
      labels:
        app: live-mutex-rs-raft
    spec:
      automountServiceAccountToken: false
      terminationGracePeriodSeconds: 30
      securityContext:
        runAsNonRoot: true
        runAsUser: 65532
        runAsGroup: 65532
        fsGroup: 65532
        fsGroupChangePolicy: OnRootMismatch
      containers:
        - name: live-mutex-rs-raft
          image: docker.io/oresoftware/live-mutex-rs-raft:0.1.127
          imagePullPolicy: IfNotPresent
          securityContext:
            allowPrivilegeEscalation: false
            runAsNonRoot: true
            seccompProfile:
              type: RuntimeDefault
            capabilities:
              drop:
                - ALL
          env:
            - name: POD_NAME
              valueFrom:
                fieldRef:
                  fieldPath: metadata.name
            - name: POD_NAMESPACE
              valueFrom:
                fieldRef:
                  fieldPath: metadata.namespace
            - name: LMX_CONFIG
              value: /etc/dd-rust-network-mutex/lmx.toml
            - name: LMX_RAFT_NODE_ID
              value: "$(POD_NAME)"
            - name: LMX_RAFT_ADVERTISE_ADDR
              value: "$(POD_NAME).live-mutex-rs-raft-peers.$(POD_NAMESPACE).svc.cluster.local:7980"
            - name: LMX_RAFT_DATA_DIR
              value: /var/lib/dd-rust-network-mutex/raft
            - name: LMX_RAFT_PEER_TOKEN
              valueFrom:
                secretKeyRef:
                  name: live-mutex-rs-raft-secrets
                  key: LMX_RAFT_PEER_TOKEN
                  optional: true
            - name: LMX_LOG_FORMAT
              value: text
            - name: RUST_LOG
              value: info,lmx=info
          ports:
            - name: http
              containerPort: 6971
            - name: raft
              containerPort: 7980
          volumeMounts:
            - name: config
              mountPath: /etc/dd-rust-network-mutex
              readOnly: true
            - name: raft-data
              mountPath: /var/lib/dd-rust-network-mutex/raft
          startupProbe:
            httpGet:
              path: /healthz
              port: http
            periodSeconds: 5
            failureThreshold: 60
          readinessProbe:
            httpGet:
              path: /raft/leaderz
              port: http
            periodSeconds: 2
            timeoutSeconds: 2
            failureThreshold: 2
          livenessProbe:
            httpGet:
              path: /healthz
              port: http
            periodSeconds: 30
            timeoutSeconds: 5
            failureThreshold: 3
          resources:
            requests:
              cpu: 100m
              memory: 192Mi
            limits:
              cpu: '2'
              memory: 1Gi
      volumes:
        - name: config
          configMap:
            name: live-mutex-rs-raft-config
  volumeClaimTemplates:
    - metadata:
        name: raft-data
      spec:
        accessModes: [ReadWriteOnce]
        resources:
          requests:
            storage: 1Gi
```

With this layout:

- Peer RPC uses `live-mutex-rs-raft-peers` and does not depend on
  leader readiness.
- Client HTTP uses `live-mutex-rs-raft`; because readiness checks
  `/raft/leaderz`, the Service normally routes to the quorum-fresh
  leader. If you prefer round-robin plus follower proxying, use
  `/healthz` for readiness instead.
- The regular `deployment/live-mutex-rs` remains untouched.
- Live smoke tests can target the client Service:

```bash
LMX_LIVE_RAFT_HTTP=live-mutex-rs-raft.default.svc.cluster.local:6971 \
LMX_LIVE_RAFT_METRICS_ENDPOINTS=live-mutex-rs-raft-0.live-mutex-rs-raft.default.svc.cluster.local:6971,live-mutex-rs-raft-1.live-mutex-rs-raft.default.svc.cluster.local:6971,live-mutex-rs-raft-2.live-mutex-rs-raft.default.svc.cluster.local:6971 \
  cargo test --test k8s_raft_live_smoke -- --ignored --nocapture
```

The metrics endpoint list should contain one unique stable pod or per-pod
Service HTTP endpoint per Raft node. The hardening gate and soak wrapper require
that list by default when `RUN_K8S_RAFT_LIVE=true`; direct cargo smoke runs can
omit it for behavior-only evidence.

The Kubernetes-driven failover smoke is intentionally fail-fast when the
control plane or Raft Service is unreachable: each live HTTP request is capped
by `LMX_LIVE_RAFT_HTTP_REQUEST_TIMEOUT_MS=10000`, it passes
`--request-timeout=5s` to `kubectl`, and kills any still-running `kubectl` child
after 15 seconds. Override those bounds with
`LMX_LIVE_RAFT_HTTP_REQUEST_TIMEOUT_MS=30000`,
`LMX_LIVE_RAFT_KUBECTL_REQUEST_TIMEOUT=10s`, and
`LMX_LIVE_RAFT_KUBECTL_PROCESS_TIMEOUT_MS=30000`; set
`LMX_LIVE_RAFT_KUBECTL_REQUEST_TIMEOUT=none` only when a specific kubectl plugin
or proxy cannot tolerate the global request-timeout flag.

The ignored Raft live smoke also has an opt-in failover path. It observes the
leader through `/raft/status`, records a bounded no-wait acquire/release
history through the Service, deletes that leader pod with `kubectl` while more
no-wait traffic is in flight, waits for the pod UID to change so the test cannot
pass before Kubernetes actually replaces the pod, waits for the Service to
expose a leader again, checks the recorded single-key history for
linearizability, proves acquire/release still works through the Service, and
then waits for the StatefulSet rollout to settle:

```bash
LMX_LIVE_RAFT_HTTP=live-mutex-rs-raft.default.svc.cluster.local:6971 \
LMX_LIVE_RAFT_METRICS_ENDPOINTS=live-mutex-rs-raft-0.live-mutex-rs-raft.default.svc.cluster.local:6971,live-mutex-rs-raft-1.live-mutex-rs-raft.default.svc.cluster.local:6971,live-mutex-rs-raft-2.live-mutex-rs-raft.default.svc.cluster.local:6971 \
LMX_LIVE_RAFT_NAMESPACE=default \
LMX_LIVE_RAFT_STATEFULSET=live-mutex-rs-raft \
LMX_LIVE_RAFT_KUBECTL_FAILOVER=true \
  cargo test --test k8s_raft_live_smoke -- --ignored --nocapture
```

## Authentication

When `LMX_AUTH_TOKEN` is set in the broker's env, every TCP/UDS
connection must send a `{"type":"auth","uuid":"...","token":"..."}`
frame as its first message, and every HTTP request must carry an
`Authorization: Bearer <token>` (or `X-LMX-Auth: <token>`) header.

Create the secret with whatever rotation pipeline you already use
(External Secrets, sealed-secrets, sops, ArgoCD vault plugin, etc.).
A minimal example with the in-cluster `Secret` resource:

```bash
kubectl create secret generic live-mutex-rs-secrets \
  --from-literal=LMX_AUTH_TOKEN="$(openssl rand -hex 32)" \
  --namespace default
```

Inside the cluster you usually pair this with a `NetworkPolicy`
that only allows the namespaces that need locking to talk to the
broker on `lmx-tcp` / `http`:

```yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: live-mutex-rs-allow
  namespace: default
spec:
  podSelector:
    matchLabels:
      app: live-mutex-rs
  policyTypes: [Ingress]
  ingress:
    - from:
        - podSelector: {}            # any pod in the same namespace
        - namespaceSelector:
            matchLabels:
              role: locking-client    # other namespaces opt in
      ports:
        - { protocol: TCP, port: 6970 }
        - { protocol: TCP, port: 6971 }
```

## TLS

The broker itself can terminate TLS via the optional `tls` cargo
feature:

- `LMX_TLS_CERT` — PEM-encoded server certificate path
- `LMX_TLS_KEY` — PEM-encoded server private key path

Both files normally come from a Kubernetes `Secret` mounted as a
volume. In a typical service-mesh-fronted cluster (Istio, Linkerd,
Cilium) the mesh terminates mTLS upstream of the broker pod, so
the in-broker TLS is rarely needed.

## Scaling, durability, and HA

### Regular Broker: why single-replica

For the regular non-Raft Broker, all lock state lives in one process memory:
holders, queues, fencing-token counters, deadline BTreeMap, and the
partial-grant tracker. Two regular Broker replicas would each have their own
state, so:

- A client connecting to replica A and a client connecting to
  replica B would never see each other's locks (split brain).
- Service-level mutual exclusion would silently degrade, which is
  the worst possible failure mode for a locking service.

Therefore the regular Broker deployment uses 1 replica, `Recreate` strategy,
and no `HorizontalPodAutoscaler`.

### Regular Broker pod restarts and lock loss

A regular Broker pod restart drops all in-memory state. Holders that were
holding locks at the moment of restart get a `connection reset` on their TCP
socket; they are responsible for re-acquiring on reconnect, and the regular
broker will mint **new** fencing tokens. Use TTLs (default 4 s) so callers that
don't reconnect promptly free up their slots naturally.

If you need the lock service to survive a broker crash, the right answer is one
of:

1. **BrokerRaft** as a separate three- or five-node StatefulSet/deployment path.
   BrokerRaft uses leader election, quorum commit, replicated logs, and
   deterministic grant metadata for lock UUID/fencing-token replay; see
   `## High availability` in the readme and [`docs/raft.md`](raft.md) for the
   state and sequence diagrams.
2. **Active-passive HA** behind a single-leader gate (e.g. a
   Postgres advisory lock or a Kubernetes `Lease`). Only the leader
   serves clients; the passive replica picks up if the leader's
   `Lease` lapses. This is a legacy design option for the regular Broker:
   fencing tokens reset on failover, so callers must be prepared to see the
   counter restart.

In practice, the single-replica + `Recreate` posture has been
sufficient for our production workloads; the broker restarts in
under 200 ms and clients reconnect on the next acquire.

### Resource sizing

The reference resource block above (`50m`/`96Mi` requests, `1`/`1Gi`
limits) handles a steady-state workload of a few thousand acquires
per second on a hot key plus tens of thousands of cold-key holders.
The broker is single-threaded for state mutations (it uses
`parking_lot::Mutex` over the lock-state map), so giving it more
than ~2 vCPU rarely buys throughput; raise the limit if you observe
the Tokio runtime starving on the I/O side under burst load.

The most useful Prometheus series for sizing decisions are:

- `dd_rust_network_mutex_concurrent_clients` — open TCP/UDS
  connections.
- `dd_rust_network_mutex_request_duration_seconds` — fixed-route latency
  buckets for Broker HTTP, BrokerRaft HTTP, and synchronous persistent
  socket-frame handling.
- `dd_rust_network_mutex_request_payload_bytes` — persistent TCP/UDS JSON
  frame-size buckets for correlating CPU profiles with payload pressure.
- `dd_rust_network_mutex_pending_deadlines` — outstanding TTL
  deadlines (a backlog here means callers are dying without
  releasing).
- `dd_rust_network_mutex_ttl_evictions_total` — counter of forced
  releases by the periodic sweeper. Sustained growth means
  misbehaving callers.

## Observability

The broker emits structured logs to stdout (text by default; set
`LMX_LOG_FORMAT=json` for JSON), with every log line tagged with the
`routine_id` that produced it (see the readme for the convention).
Routine IDs are static literals embedded in source, so a
`kubectl logs … | rg ddl-routine-XYZ` lands you at the exact
function in this crate.

If `OTEL_EXPORTER_OTLP_ENDPOINT` is set, the broker installs a
`tracing-opentelemetry` exporter that ships every `tracing` span /
event over OTLP/gRPC. To enable in-cluster:

```yaml
env:
  - name: OTEL_EXPORTER_OTLP_ENDPOINT
    value: http://otel-collector.observability.svc.cluster.local:4317
  - name: OTEL_SERVICE_NAME
    value: live-mutex-rs
  - name: OTEL_RESOURCE_ATTRIBUTES
    value: deployment.environment=prod,service.namespace=default
```

The OTel exporter can be disabled at runtime via the
`/admin/otel` HTTP endpoint (POST with the admin auth header) — see
the readme's "Observability" section.

## Operator runbook

### Status page

`GET /` (and `GET /status`) on `:6971` serves a server-rendered HTML
operator page: connected clients, holders by key, queued waiters,
pending deadlines, TTL-eviction counter, and the embedded
`/metrics` exposition. Auto-refreshes every 5 s. No JS, no external
assets, friendly to `curl | rg`. To expose it operators-only,
set `LMX_STATUS_PORT=…` to bind a separate read-only listener that
serves only the status / `/healthz` / `/readyz` / `/metrics` paths.

### Forcing a restart

If you suspect the in-memory state has wedged (e.g. a holder is
stuck on a key the sweeper never frees because the holder is still
TCP-connected and answering keepalives), you can:

```bash
kubectl rollout restart deployment/live-mutex-rs
```

`Recreate` strategy means the old pod terminates first, then the
new one starts. Plan a brief acquire-error window when you do
this.

### Local smoke test

To sanity-check a freshly applied manifest from a developer
workstation:

```bash
kubectl port-forward svc/live-mutex-rs 16970:6970 16971:6971 &

# HTTP healthcheck
curl -s http://127.0.0.1:16971/healthz

# Acquire + release a real lock
LOCK_UUID=$(curl -s http://127.0.0.1:16971/v1/lock \
  -H 'content-type: application/json' \
  -H "Authorization: Bearer $LMX_AUTH_TOKEN" \
  -d '{"key":"smoke","ttlMs":2000}' | jq -r .lockUuid)

curl -s http://127.0.0.1:16971/v1/unlock \
  -H 'content-type: application/json' \
  -H "Authorization: Bearer $LMX_AUTH_TOKEN" \
  -d "{\"key\":\"smoke\",\"lockUuid\":\"$LOCK_UUID\"}"
```

If both calls return `200 OK` with `acquired: true` and
`unlocked: true` respectively, the broker is healthy.

## Multi-cluster / multi-region

The broker is **regional by design**: it serves whatever cluster it
runs in. To coordinate across clusters or regions, run one broker
per cluster and let your application use the local one — distributed
locks across regions need a different tool (Postgres advisory locks,
etcd, ZooKeeper, Redis Redlock with all of its caveats). A single
broker stretched across regions would have RTT latencies that
defeat the purpose of having a fast in-memory locking service.
