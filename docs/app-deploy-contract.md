# App deployment contract — what an org repo may declare, and what k8s-cluster owns

_The enforceable boundary between the 7 app orgs and this platform repo. Companion to
[gitops-boundary-audit.md](gitops-boundary-audit.md). Reference implementation:
`remote/deployments/3fa-backend/deploy/k8s/`._

The boundary is a **contract**, not a repo boundary: split by who owns the lifecycle. If a
platform-team change (upgrade ingress, rotate a store) requires an app-repo PR, or an
app-team change (add a sidecar) requires a k8s-cluster PR, the seam is in the wrong place.

## Layer 1 — this repo (platform) owns

- **Cluster-scoped + shared infra:** CNI, ingress/gateway, cert-manager, external-secrets +
  `ClusterSecretStore/dd-cluster-secrets`, Prometheus/observability, StorageClasses, CRDs,
  operators (KEDA, ESO, Strimzi, …).
- **Tenancy objects, per app:** `Namespace`, `ResourceQuota`, `LimitRange`, a default-deny
  `NetworkPolicy`, `ServiceAccount`/RBAC, and the Argo **`AppProject`**.
- **Registration:** an `Application` (or ApplicationSet entry) that *points at the app repo*
  — repoURL, path, targetRevision. A pointer, not the app's manifests.

## Layer 2 — the app org repo owns

A `deploy/k8s/` directory containing **only namespace-scoped resources**:

- `Deployment` / `StatefulSet` (workload only — not shared cluster-formation cores; see
  fiducia below), `Service`, `HPA`, `Ingress`/`HTTPRoute`, `ConfigMap`.
- `ExternalSecret` referencing `ClusterSecretStore/dd-cluster-secrets` at a path scoped to
  the org. **No plaintext secrets in either repo.**
- Cloud resource *claims* (if using Crossplane later), never raw cloud CRDs/providers.

**An app repo MUST NOT declare:** a `Namespace`, any `ClusterRole`/`ClusterRoleBinding`,
CRDs, StorageClasses, operators, or ingress controllers. Those are Layer 1. (Note: this is
stricter than today's `3fa-backend`, which ships its own `namespace.yaml` — see migration.)

## The registration pointer (copy this shape)

```yaml
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: <app>
  namespace: argocd
spec:
  project: <app>                       # its own AppProject, NOT default
  source:
    repoURL: git@github.com:<org>/<app-repo>.git   # the APP repo, not k8s-cluster
    targetRevision: <git-tag>          # promotion knob — pin prod to a tag, not main/dev
    path: deploy/k8s
  destination:
    server: https://kubernetes.default.svc
    namespace: <app>                   # its own namespace, NOT default
  syncPolicy:
    automated: { prune: true, selfHeal: true }
    syncOptions: [ ServerSideApply=true ]
```

## ⚠️ Submodules are inventory, NOT a render source

ArgoCD's repo-server runs with **`reposerver.enable.git.submodule=false`** (the
argocd-submodule-init incident). It checks out k8s-cluster **without submodule contents**, so
any Application whose `path` points inside a gitlink renders **empty**.

Three Applications are already broken this way — their `path` resolves to zero tracked files:

| Application | dead path | fix |
|---|---|---|
| `dd-billing-server` | `remote/deployments/billing-server-rs/k8s/ec2` | point `repoURL` at `quaestor-ledger/quaestor-monorepo`, `path: apps/billing-server.rs/k8s` |
| `dd-dart-server` | `remote/deployments/dart-server/k8s/ec2` | point `repoURL` at its own repo |
| `dd-gleam-lambda-runner` | `remote/deployments/gleam-lambda-runner/k8s/ec2` | point `repoURL` at its own repo |

**The rule when leaning on org monorepos:** vendor the org monorepo as a submodule
(`remote/deployments/<org>-monorepo`) for **inventory, pinning, local dev, and digest
promotion** — but point the Application's `repoURL` at the **monorepo upstream directly**,
with `path: apps/<app>/k8s`. The gitlink is the *pin record*; Argo reads upstream. CI promotes
by bumping the gitlink and `targetRevision` together.

This is why the 53 working Applications work: their manifests are duplicated as real files
under `remote/argocd/`. Moving to monorepo-sourced apps removes that duplication — but only if
`repoURL` targets the upstream, never the submodule path.

Worked reference: **daedalus** (`remote/argocd/apps/daedalus.applications.yaml` +
`projects/daedalus.{tenant,appproject}.yaml`, manifests in
`daedalus-monorepo/apps/*/k8s/`).

## The AppProject is what makes it enforced (not a suggestion)

One `AppProject` per app pins where it can deploy and forbids cluster-scoped resources.
Template lives at `remote/argocd/projects/_template.appproject.yaml`. With
`clusterResourceWhitelist: []`, if an app repo commits a ClusterRole or tries to sync into
another namespace, **Argo rejects it at sync time**. That is the difference between a
boundary and a naming convention you violate under deadline pressure.

Because Layer 1 owns the `Namespace`, the app repo does **not** create it and does **not**
need `CreateNamespace=true`.

## Secrets

Platform owns ESO + `ClusterSecretStore/dd-cluster-secrets`. Apps only ever commit an
`ExternalSecret` pointing at a store path scoped to their org (e.g. AWS Secrets Manager
`dd/remote-dev/<app>-secrets`). See `remote/deployments/3fa-backend/deploy/k8s/externalsecret.yaml`.

## Cloud resources (DNS, DBs, buckets)

- **Edge/DNS/WAF (Cloudflare):** stays in the org's own `-infra` repo (athleto-infra,
  daedalus-infra, quaestor-infra Cloudflare modules). This is a **disjoint plane** — it
  declares no k8s objects and cannot collide. Only rule: don't flip a proxy/DNS cutover
  before the cluster-side Ingress + cert exist (ACME HTTP-01 ordering).
- **Databases:** Supabase/Postgres reached over egress; connection via `ExternalSecret`.
  Never a raw RDS/cloud CRD in `deploy/k8s`.

## Per-org disposition (hybrid ownership)

| Org | k8s workload on this cluster? | Ownership target |
|---|---|---|
| 3fa | yes (`threefa` ns) | **already the reference** — app-repo `deploy/k8s` |
| athleto | `dd-athleto-backend` | app-repo ownership (`athleto-backend.rs/k8s`), own namespace; delete monorepo dup |
| quaestor | billing + web servers | app-repo `deploy/k8s`; replace hand-copied CRs with an ApplicationSet entry |
| **fiducia** | **api/web servers ONLY** | app-repo ownership; **node/brain never here** (see below) |
| benefactor | `benefactor-backend-rs` | app-repo ownership |
| canonical | web server + browser runners | monorepo-managed here is fine; keep pointer clean |
| claritas | `dd-data-viz-rs` | monorepo-managed here is fine |
| daedalus | fabrication/api/web (not yet) | scaffold `deploy/k8s` in own namespace when they land |

## fiducia — component split (critical)

`fiducia-node.rs` and `fiducia-brain.rs` (the Raft data-plane + brain) deploy **only on
fiducia-infra's own multi-cloud clusters**. They are **never** a tenant here. The parts that
belong on k8s-cluster are the **stateless generic Rust API + web servers**, which consume
the Raft mesh over the network. The existing `remote/argocd/fiducia(-hetzner)` StatefulSets
are a divergent second copy of the core and are being retired (see the audit's fiducia
section for the safe repoint-then-prune sequence).

## Anti-patterns (from the reference material, all currently present)

- App manifests living in the cluster repo → every deploy is a cross-team PR. _(46/61 today.)_
- App repos creating namespaces/CRDs → the privilege boundary collapses.
- Everything in `namespace: default` → one shared blast radius. _(106 manifests today.)_
- Hand-copying Application CRs between repos → drift. _(quaestor's triple dd-billing-server.)_
- One giant umbrella chart → coupled release cycles.
