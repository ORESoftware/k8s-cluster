# `remote/deployments/thread-operator-go`

`dd-thread-operator` is a Go Kubernetes controller that owns the lifecycle of one
per-thread workspace pod as a **declarative custom resource** (`Thread` in `dd.dev/v1alpha1`).

## Why this exists

Per-thread provisioning today is split across:

- `remote/k8s/0[6-9]-thread-*.template.yaml` — hand-templated PVC/Deployment/Service/Ingress.
- Vercel control plane (`createK8sPod`) — substitutes `{THREAD_ID}` / `{THREAD_ID_SHORT}` / `{USER_ID}` and `kubectl apply`s.
- `dd-idle-reaper` (Rust) — calls Vercel's sweep route to scale idle thread Deployments to `replicas=0`.
- `/u/admin/k8s` (Next.js admin dashboard) — sleep / wake / delete buttons.

Each of those agrees on the same naming + label conventions, but the contract lives in three
places. The operator collapses the contract into one CRD: `kubectl apply` a `Thread`, get a
working thread pod; `kubectl delete` the `Thread`, the workspace tears down via owner-ref GC.

## Safety contract (v1alpha1, opt-in)

The operator is **strictly opt-in**. Concretely:

- The reconciler ONLY acts on `Thread` custom resources. Existing template-provisioned threads
  do not produce `Thread` CRs and are therefore never touched.
- Every child resource the operator creates carries
  `dd.dev/managed-by=dd-thread-operator`. If the operator finds an existing object with the
  same name as a child it expects to manage but the label is missing, it **refuses to mutate
  it** and surfaces an `UnmanagedConflict` condition on the `Thread` status. That makes
  accidental adoption of a template-provisioned thread structurally impossible.
- Owner references (`controller=true`) are set on every child so deletion of a `Thread`
  cascades correctly via Kubernetes garbage collection.

The `idle-reaper-rs` and `dev-server` Node provisioning paths keep working unchanged. When you
are ready to migrate one thread to operator management, end its current thread, then apply a
`Thread` CR with the same `threadId`. The resulting resources will have the same names and
labels as before, plus the `dd.dev/managed-by` label, and the admin K8s dashboard will see
them transparently.

## Custom resource

```yaml
apiVersion: dd.dev/v1alpha1
kind: Thread
metadata:
  name: sample-thread
  namespace: dd-dev
spec:
  threadId: 11111111-1111-4111-8111-111111111111
  threadIdShort: sample01           # optional; derived from threadId when omitted
  userId: 00000000-0000-0000-0000-000000000001
  ingressHost: agents.example.com
  image: docker.io/library/dd-dev-server:dev
  desiredState: Running             # Running | Sleeping
  workspaceSize: 5Gi
  idleTimeoutSeconds: 1800          # auto-sleep after 30 min idle
  ttlSecondsAfterIdle: 86400        # auto-delete after 1 day idle
  # lastActivityAt: 2026-05-21T20:11:13Z   # set by external dispatchers
```

What the operator does on apply:

1. Creates a `PersistentVolumeClaim` named `dd-thread-<short>` (5Gi, RWO, default storage class).
2. Creates a `Deployment` named `dd-thread-<short>` with replicas derived from `desiredState` +
   the idle-timeout policy.
3. Creates a ClusterIP `Service` named `dd-thread-<short>` selecting `dd/threadId=<threadId>`.
4. Creates an `Ingress` named `dd-thread-<short>` with the path
   `/dd-thread/<short>(/.*)?` on `ingressHost` and SSE-friendly proxy timings (matches
   `09-thread-ingress.template.yaml`).
5. Patches `status.phase`, `status.replicasReady`, `status.podIP`, `status.conditions`.

The reconciler runs every 30 seconds even when nothing has changed, so the idle-timeout policy
is evaluated continuously without external triggers.

## Sleep / wake / GC

| Action          | How                                              | Effect                                                                          |
| --------------- | ------------------------------------------------ | ------------------------------------------------------------------------------- |
| Sleep           | `kubectl patch thread … -p '{"spec":{"desiredState":"Sleeping"}}'` | Deployment scaled to 0; PVC retained.                                           |
| Wake            | Same patch with `Running`.                       | Deployment scaled to 1; workspace remounts.                                     |
| Auto-sleep      | `idleTimeoutSeconds` + `lastActivityAt`.         | Replicas forced to 0 once `(now - lastActivityAt) > idleTimeoutSeconds`.        |
| TTL-based GC    | `ttlSecondsAfterIdle`.                           | Thread CR deleted once idle for that long; cascades to PVC/Deployment/Svc/Ing. |
| Manual delete   | `kubectl delete thread <name>`.                  | Same cascade — PVC included.                                                    |

The operator NEVER writes to `spec.lastActivityAt`. External dispatchers (Vercel, dev-server,
or whatever replaces them) update that field; the operator only reads it.

## RBAC posture (least privilege)

- Cluster-scoped: read/write on `threads.dd.dev` and event creation only.
- Namespace-scoped (`dd-dev`): CRUD on Pods (read), PVCs, Services, Deployments, Ingresses, and
  Events.
- No access to Secrets, ConfigMaps, ServiceAccounts, or other namespaces. To manage threads in
  another namespace, add another `RoleBinding` of `dd-thread-operator-children` pointing at
  this same `ServiceAccount`. Do not promote the resource Role to a `ClusterRole`.

See `k8s/ec2/01-rbac.yaml`.

## Build + run locally

```bash
go vet ./...
go test ./...
go build -o /tmp/dd-thread-operator ./cmd/operator
/tmp/dd-thread-operator --help
```

To run against a real cluster, point `KUBECONFIG` at the cluster and run the binary on your laptop:

```bash
KUBECONFIG=$HOME/.kube/config /tmp/dd-thread-operator \
  --metrics-bind-address=:9101 \
  --health-probe-bind-address=:9102 \
  --leader-elect=false
```

Then in another shell:

```bash
kubectl apply -f k8s/ec2/00-crd-thread.yaml
kubectl create namespace dd-dev || true
kubectl apply -f k8s/sample-thread.yaml
kubectl get thread -n dd-dev sample-thread -w
```

## Cluster deploy (EC2)

Apply via Argo CD — the `Application` manifest is at
[`remote/argocd/apps/dd-thread-operator.application.yaml`](../../argocd/apps/dd-thread-operator.application.yaml)
and points at this directory's `k8s/ec2` overlay.

The deployment runs the canonical `golang:1.23-bookworm` base image and builds the operator
binary at startup from a hostPath mount of this directory — same pattern as `dd-idle-reaper`
and `gcs-router`. When ECR is wired up, swap the deployment to use the multi-stage `Dockerfile`
in this directory and remove the hostPath build dance.

## Migration plan (when ready, NOT now)

The operator is opt-in by design. A future migration would look like:

1. Verify the operator is healthy on a few canary `Thread` CRs.
2. Update `dev-server`'s `createK8sPod` to create a `Thread` CR instead of raw resources.
3. Add a one-shot job that re-labels existing template-provisioned thread resources with
   `dd.dev/managed-by=dd-thread-operator` and creates matching `Thread` CRs whose spec mirrors
   their current shape; the operator will adopt them at the next reconcile.
4. Once every live thread is operator-managed, retire the `idle-reaper-rs` sweep loop's HTTP
   path and let the operator's `idleTimeoutSeconds` policy own auto-sleep. Keep
   `idle-reaper-rs` for everything else (NATS watchdog, runtime floor, cluster doctor,
   worker-image cron) — those responsibilities are unrelated to thread sleep.
5. Decommission the `0[6-9]-thread-*.template.yaml` templates as documentation only.

None of those steps are required for v1alpha1; the operator can sit unused indefinitely
without breaking anything.
