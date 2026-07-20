# GitOps boundary audit — k8s-cluster vs. the 7 app orgs

_Audited 2026-07-19. Scope: how independently-declared infra in athlet-o, benefactor.cc,
fiducia.cloud, daedalus-fab, quaestor-ledger, canonical.cloud, and claritas-viz interacts
with the central ArgoCD platform in `ORESoftware/k8s-cluster`._

## TL;DR verdict

The target model (from the reference material) is a **contract boundary**, not a repo
boundary:

- **Layer 1 — cluster repo (platform):** cluster-scoped + shared infra, and the *tenancy*
  objects per app (Namespace, ResourceQuota, LimitRange, NetworkPolicy default,
  ServiceAccount/RBAC, Argo `AppProject`), plus a **registration** (ApplicationSet /
  Application pointer) at each app repo. Pointers, not manifests.
- **Layer 2 — app repos:** only namespace-scoped resources (Deployment, Service, HPA,
  Ingress/HTTPRoute, ConfigMap, `ExternalSecret`, cloud *claims*). No CRDs, no
  ClusterRoles, no namespace creation.
- The seam is one-directional and enforced by an `AppProject` per app whose
  `destinations` = that app's namespace and `clusterResourceWhitelist: []`.

**Where we actually are:**

| Signal | Target | Current |
|---|---|---|
| App manifests live in… | app repos | **k8s-cluster** (46/61 Applications set `repoURL` to k8s-cluster itself) |
| Tenant isolation | namespace per app | **106 workload manifests hardcode `namespace: default`** |
| Enforced boundary | `AppProject` per app | **1 AppProject total** (`ai-ml-platform`); everything else `project: default` |
| App registration | ApplicationSet | hand-written Application per app; onboarding = hand-copy |
| repoURL | uniform | **mixed SSH + HTTPS** (fiducia already needs a per-cluster special-case) |

So we've built the **"central config monorepo"** variant — a legitimate topology, and one
app (`3fa-backend`, below) already does it the target way — but the boundary is *convention,
not enforcement*, and `default` is a shared blast radius. The new orgs are mostly safe
because they externalize deployment to us; the sharp risks are (1) fiducia's parallel
GitOps hub, (2) duplicate/triple ArgoCD Application declarations, and (3) the missing
`AppProject` guardrails.

## The reference pattern already in-repo

`remote/deployments/3fa-backend/deploy/k8s/` is the target contract, proven:

- own namespace `threefa` (`namespace.yaml`), not `default`
- `ExternalSecret` → the shared `ClusterSecretStore/dd-cluster-secrets` (`externalsecret.yaml`)
- its own `NetworkPolicy`
- Application `repoURL: git@github.com:3FA-app/3fa-backend.rs.git`, `path: deploy/k8s` —
  **points at the app repo**, not at k8s-cluster.

Everything below is "make more things look like `3fa-backend`, and make an `AppProject`
force it."

## Per-org taxonomy

### Category 1 — Cloudflare-edge-only (SAFE, disjoint plane)

**athleto-infra**, **daedalus-infra** — Terraform (Cloudflare DNS/WAF) and/or a single
Cloudflare Worker. Zero k8s objects, no ArgoCD, explicitly cede the cluster to us. No
controller can collide because they declare nothing in-cluster. Only coupling is
*ordering/assumption drift* (e.g. athleto flipping `proxy_app_origins` before cert-manager
issues a cert would deadlock ACME HTTP-01).

- Latent risk in athlet-o: `athleto-backend.rs/k8s/ec2/` (and an exact duplicate in
  `athleto-monorepo/apps/…`) hardcodes a `dd-athleto-backend` Deployment+Service in
  `namespace: default` — double-owned if our App-of-Apps also renders it.

### Category 2 — Parallel GitOps hub (CRITICAL — see dedicated section)

**fiducia-infra** — a full kustomize + Terraform + **its own ApplicationSet** that points
at its own repo and its own clusters. Overlaps our `remote/argocd/fiducia(-hetzner)` on
identically-named objects with **divergent specs**.

### Category 3 — Satellite / generator (MEDIUM — coupling + drift)

**quaestor-infra** — `topology.toml` → `render.mjs` generates ArgoCD Application CRs meant
to be **hand-copied into k8s-cluster**, plus NetworkPolicies scoped into our shared
`default`, plus Cloudflare DNS. Consequences:

- `dd-billing-server` Application is now declared **three times** (our
  `apps/dd-billing-server.application.yaml`, inline in `clusters/aws/applications.yaml`,
  and quaestor's generated CR) — last-applied-wins, and quaestor's copy **drops
  `ServerSideApply=true`**.
- `dd-quaestor-web-server` Application is generated pointing at
  `remote/deployments/quaestor-web-server-rs/k8s/ec2`, **a path that doesn't exist here yet**
  → would sync-error if copied in.
- The billing workload itself is the *same* submodule (`quaestor-ledger/billing-server.rs`
  = `remote/deployments/billing-server-rs`), so the hazard is two/three auto-pruning
  Applications fighting over one object set, not a rival server.

### Category 4 — Thin / deferred (SAFE in-cluster; shared state at the edges)

**canonical.cloud**, **benefactor.cc**, **claritas-viz** — no k8s/Helm/Terraform/ArgoCD in
their own trees. They hand digests to us and deploy via Pages/Workers/Supabase. Real shared
surfaces are *data/edge*, not k8s objects:

- **benefactor:** Cloudflare Worker `go.benefactor.cc` and our `benefactor-backend-rs` are
  **dual writers to the same Supabase table** (`benefactor_outreach_clicks`) sharing one
  HS256 secret.
- **canonical:** app server hard-depends on in-cluster `dd-otel-collector.observability`
  and on our `canonical-browser-runner-set` scale set.
- **claritas:** shared Supabase telemetry project + shared `fiducia.cloud` secrets/leases
  plane; the actual `dd-data-viz-rs` workload is ours alone.

## Concrete collision inventory (the set to fix)

| # | Resource | Declared by | Risk |
|---|---|---|---|
| 1 | `Application/dd-billing-server` (ns argocd) | k8s-cluster ×2 + quaestor-infra generated | triple declaration; SSA dropped in one |
| 2 | `Application/dd-quaestor-web-server` | quaestor-infra (path missing here) | sync-error if landed |
| 3 | `Deployment/Service dd-athleto-backend` (ns default) | **live copy** = `remote/argocd/dd-next-runtime/`; `athleto-backend.rs/k8s/ec2` + athleto-monorepo dup are **stale, undeployed** | not double-owned *today* (no Application points at the app-repo path) — but 3 divergent copies of one workload; wiring up the app-repo copy would instantly collide in `default` |
| 4 | `Namespace/fiducia` | fiducia-infra (`enforce=privileged`) + us (`enforce=baseline`) | conflicting PSA labels |
| 5 | `StatefulSet/fiducia-node`,`fiducia-brain` | fiducia-infra + us | divergent replicas/serviceName/Raft topology |
| 6 | `Service/fiducia-load-balance` (LoadBalancer) | fiducia-infra + us | both provision a cloud LB |
| 7 | NetworkPolicies in ns `default` | quaestor-infra + us | correctness relies on `part-of` label discipline |
| 8 | `dd-fabrication-server` in `default` | already scraped by our Prometheus; new `fabrication-server-rs` checkout has no `deploy/k8s` yet | if authored to the `default` convention, name/port(8113) clash |

## fiducia — CRITICAL: split by COMPONENT, not by cluster

The boundary here is per-component. fiducia's **stateful Raft core does not belong on
k8s-cluster at all**; its **stateless generic servers do**.

- **Owned exclusively by fiducia-infra, on its own clusters (hetzner/civo/vultr):**
  `fiducia-node.rs` and `fiducia-brain.rs` — the Raft data-plane + brain. Cross-cloud mesh,
  one Raft member *per cluster*, shard count 256, RF3, peers over public DNS,
  `ghcr.io/fiducia-cloud/*` images, hard cross-host anti-affinity. This is a
  cluster-formation topology that only makes sense on fiducia's own multi-cloud fleet.
- **Legitimately on k8s-cluster (Layer-2 tenants):** fiducia's **generic Rust API server and
  Rust web server** — stateless, namespace-scoped, standard `3fa-backend`-style contract.
  They consume the Raft core over the network (public DNS to the mesh); they do not form part
  of the quorum.

**The current problem:** `remote/argocd/fiducia` + `fiducia-hetzner` deploy the
`fiducia-node`/`fiducia-brain` **StatefulSets** here — i.e. k8s-cluster is running a *second,
divergent* copy of the very core that fiducia-infra owns (all 3 members in one cluster,
`rust:1.95-bookworm` in-pod build vs the mesh's pinned images; conflicting `fiducia`
Namespace PSA label `baseline` vs `privileged`; `serviceName` `fiducia-node-peer` vs
`fiducia-node`). Both hubs run `prune:true, selfHeal:true`, held apart only by fiducia-infra's
`environment=nonproduction` label gate — one mislabel from two controllers mutually pruning a
Raft cluster.

**Hardening rule for fiducia (do not fold the repos together):**

1. **Remove the Raft core from k8s-cluster.** Delete/retire the `fiducia-node` and
   `fiducia-brain` StatefulSets (and their LB/PDB/NetworkPolicy) from `remote/argocd/fiducia`
   and `fiducia-hetzner`, unless there is a deliberate reason to run a *local dev quorum* —
   in which case **rename** its objects (e.g. `fiducia-sat-node`) so names can never collide
   with the mesh's. The mesh, on fiducia-infra's clusters, is the single source of truth for
   the core.
2. **Keep only the generic api/web servers here**, in the `fiducia` namespace, as normal
   Layer-2 apps: their manifests should live in the fiducia app repo's `deploy/k8s`
   (`repoURL` → the app repo), governed by a `fiducia` `AppProject`, consuming the mesh via
   config/`ExternalSecret` (peer DNS, tokens) — never re-declaring node/brain objects.
3. **Belt-and-suspenders on the seam:** never register a single physical cluster into both
   ArgoCD hubs; fiducia-infra's generator must never emit a destination resolving to the
   k8s-cluster context, independent of the label gate.
4. TLS/LB secrets (`fiducia-load-balance-tls`) are out-of-band on both sides — the api/web
   tenants on k8s-cluster should not own a `fiducia-load-balance` LoadBalancer at all; that
   is a mesh concern.

### Repoint-then-prune runbook (DO NOT skip to the prune)

⚠️ The k8s-cluster `fiducia-node`/`fiducia-brain` StatefulSets are **currently the serving
quorum** for `app.fiducia.cloud` / `admin.fiducia.cloud` (backend/auth/admin talk to them
in-cluster). Deleting them from the kustomization prunes a **live** quorum. Only proceed once
a human confirms the fiducia-infra mesh is reachable from this cluster (network path + TLS +
auth). The current local wiring is:

- `fiducia-load-balance` (router) seeds from the local `Service/fiducia-node` (:8090).
- `fiducia-auth` → `FIDUCIA_KV_URL=http://fiducia-load-balance.fiducia.svc.cluster.local:8088`.
- `fiducia-admin` → `FIDUCIA_BRAIN_URL=http://fiducia-brain.fiducia.svc.cluster.local:8095`.
- `fiducia-backend`/`fiducia-auth` → `FIDUCIA_AUTH_URL=…fiducia-auth…:8097` (stays local — auth is stateless and stays here).

**Step 1 — repoint the stateless servers at the mesh (no deletion yet):**
- Point `fiducia-load-balance`'s seed / `fiducia-admin`'s `FIDUCIA_BRAIN_URL` /
  `fiducia-auth`'s `FIDUCIA_KV_URL` at the mesh endpoints
  (`node.<cloud>.fiducia.cloud:9090`, `brain.<cloud>.fiducia.cloud:8095` — the addresses in
  fiducia-infra's `clusters/<cloud>/topology.env`), delivered via `ExternalSecret`/ConfigMap,
  not hardcoded. Add the mesh CA / mTLS creds the same way.
- Verify backend/auth/admin stay green reading/writing through the mesh **while the local
  quorum is still up** (safe rollback: revert the env).

**Step 2 — only after Step 1 is verified — prune the local core:**
- Remove `fiducia-node.statefulset.yaml`, `fiducia-node.service.yaml`,
  `fiducia-node.networkpolicy.yaml`, `fiducia-brain.statefulset.yaml`,
  `fiducia-brain.networkpolicy.yaml` (and the node/brain PDBs) from
  `remote/argocd/fiducia/kustomization.yaml` and the `fiducia-hetzner` overlay. ArgoCD prunes
  the now-unreferenced quorum. Decide whether `fiducia-load-balance` stays (thin router to the
  mesh) or also goes.
- Then adopt the `fiducia` `AppProject` for the remaining stateless tenants
  (backend/auth/admin), sourced from the fiducia app repo's `deploy/k8s`.

## Central systemic gaps (independent of any single org)

1. **`default` is the blast radius.** 106 manifests hardcode it. No per-app quota, limit,
   or default-deny; a bad NetworkPolicy with `podSelector: {}` black-holes everyone.
2. **No enforced boundary.** `project: default` everywhere means nothing stops an app repo
   from committing a ClusterRole or syncing into another app's space.
3. **Onboarding is hand-copy** (quaestor's pain, and the triple-declaration). No
   ApplicationSet to register apps from a single list.
4. **repoURL is inconsistent** (SSH vs HTTPS) and mostly self-referential — the "app
   manifests in the cluster repo" anti-pattern; every app deploy is a k8s-cluster PR.
5. **`dd-next-runtime` is a 207-resource monolith** (`remote/argocd/dd-next-runtime/kustomization.yaml`),
   synced into `default` with prune+selfHeal on **both** AWS and Hetzner. It holds
   `dd-athleto-backend`, `dd-athleto-app-rs`, the fiducia stack, `threefa-sync-server`, and
   most `dd-*` workloads. **This is the single biggest structural obstacle to per-app
   tenancy** — every extraction means removing resources from this kustomization (which
   *prunes them from `default`*, i.e. an outage window) and standing them up in a new
   namespace. There is no app here that can be moved "for free."
6. **Observability hardcodes the flat `default` world.** `k8s-resource-exporter` enumerates
   ~150 `dd-*` workload names in a single allowlist
   (`remote/argocd/observability/k8s-resource-exporter.{deployment,configmap}.yaml`), and
   Prometheus rules key on `namespace="default"`. Any namespace migration must update these
   in lockstep or the workload silently drops out of monitoring/alerting.

## Phased hardening roadmap

Ordered by value/risk, safe-first. Waves mirror the reference model.

**Phase 0 — stop the bleeding (low risk, in-repo):**
- De-duplicate `Application/dd-billing-server` to a single source; delete quaestor-infra's
  hand-copied CRs and have `render.mjs` emit an ApplicationSet entry instead (#1, #2).
- Resolve the `Namespace/fiducia` PSA label conflict and decide the fiducia instance-naming
  (#4) — highest-severity operational risk.
- Rotate the plaintext `ghp_…` PAT in `benefactor.cc/env/.main.env` (see Security).
- Add a `deploy/k8s` skeleton (following `3fa-backend`) for `fabrication-server-rs` so it
  lands in its own namespace, not beside the already-scraped `dd-fabrication-server` (#8).

**Phase 1 — make the boundary real (per app, incremental):**
- Add an `AppProject` per app/org: `destinations` pinned to the app's namespace,
  `clusterResourceWhitelist: []`, `sourceRepos` pinned to that app's repo. This is the
  single highest-leverage change — it converts "separation" from convention to enforcement.
- Introduce a tenant scaffold per app (Namespace + ResourceQuota + LimitRange +
  default-deny NetworkPolicy + ServiceAccount) under a `tenants/` tree, owned by us.

**Phase 2 — migrate workloads off `default` (app by app, not big-bang):**
- Move each app's Deployment/Service/ES/NetPol into its own namespace, `3fa-backend`-style.
- Flip its Application `repoURL` to the app repo's `deploy/k8s` where the app repo is ready
  to own manifests (athleto-backend, quaestor already have the dirs).

**Phase 3 — registration + promotion:**
- Replace hand-written Applications with an ApplicationSet (list generator) keyed on
  (name, repo, revision). `targetRevision` becomes the promotion knob; pin prod to a git
  tag, not `main`/`dev`.
- Adopt sync-waves: `-2` CRDs/cert-manager, `-1` ingress/ESO/StorageClass, `0` tenants,
  `1` the app ApplicationSet.

## Security flags surfaced during the audit

- **`benefactor.cc/env/.main.env`** contains a plaintext GitHub PAT (`ghp_…`, duplicated as
  `DD_GITHUB_TOKEN`). Revoke/rotate and confirm it's gitignored. (Not echoed here.)
- **Expired PAT** already documented in `AGENTS.md` blocks the `benefactor-backend-rs`
  GitOps deploy (`default/dd-git-clone-token` + ArgoCD repo cred) — same rotation workstream.
