# Crossplane and Kubernetes Operators — Where They Fit This Cluster

Status: analysis / decision doc. No code or manifests change as part of this document.

This doc explains what operators and Crossplane actually are, inventories what we
already run, and then answers the real question: **would writing a custom operator
(or adopting Crossplane) buy us anything here?** Short answer up front:

> We already benefit heavily from *third-party* operators and should adopt one more
> (cert-manager on AWS). We should **not** write a custom operator today — every
> workload that looks operator-shaped is better served by templating
> (ApplicationSet / Kustomize) or by an existing upstream operator. Crossplane is
> a legitimate future option for our out-of-cluster resources (R2, S3, Secrets
> Manager, RDS) but at the current footprint Terraform is cheaper to keep.

---

## 1. Concepts, briefly

### The operator pattern

Kubernetes is a reconciliation machine: you declare desired state in the API
server, controllers notice drift between desired and actual, and they act to
converge. Built-in controllers do this for Deployments, Services, etc.

An **operator** extends that machine to things Kubernetes doesn't natively
understand. It has two halves:

1. **CRDs (Custom Resource Definitions)** — new API types. `kind: ExternalSecret`
   or `kind: ScaledObject` are not built into Kubernetes; the operator's CRDs
   teach the API server to store and validate them.
2. **A controller** — a long-running pod that watches those custom resources and
   reconciles the world to match them, forever, automatically.

The value proposition is *encoded operational knowledge*: instead of a human (or
a cron script) knowing "when the cert is 30 days from expiry, renew it and bounce
the gateway," a controller knows it and does it on a loop, with retries, status
reporting, and no human in the path.

The cost proposition, which matters just as much: a **custom** operator is a
production service you now own. It needs a language/framework (kubebuilder,
Operator SDK, kopf), RBAC, upgrade paths for its CRDs, testing against API-server
versions, and on-call understanding. A controller with a bug doesn't fail once
like a script — it *reconciles* its bug continuously. The break-even point is
roughly: **many instances of the same resource × frequent lifecycle events ×
logic too dynamic for templates.** Below that line, templating wins.

### Crossplane

Crossplane is a specific, pre-built family of operators that reconcile **external
cloud resources** — buckets, DNS records, managed databases, IAM users — as
Kubernetes custom resources. Think "Terraform, but as a control loop":

| | Terraform | Crossplane |
|---|---|---|
| Model | Imperative apply of a plan, human-triggered | Continuous reconciliation, drift auto-corrected |
| State | `.tfstate` file to protect and share | The Kubernetes API server *is* the state store |
| GitOps fit | Needs a wrapper (Atlantis, CI job) | Native — ArgoCD syncs `Bucket` YAML like any manifest |
| Composition | Modules | `Composition` + `CompositeResourceDefinition` (XRDs): define e.g. a `TenantDatabase` claim that expands to RDS instance + secret + firewall rule |
| Cost | Near zero runtime footprint | Control plane + one provider pod per cloud, CRD sprawl (AWS provider alone ships hundreds of CRDs), version-upgrade churn |

Crossplane shines when platform teams want app teams to self-serve cloud
resources by committing a small claim YAML, without touching Terraform or cloud
consoles. It is overkill when one person manages a handful of static resources.

---

## 2. What this cluster already does (inventory)

We are already an operator-heavy shop — all third-party, installed as ArgoCD
Applications pointing at upstream Helm charts:

| Operator | Where installed | CRDs we use |
|---|---|---|
| External Secrets Operator | `remote/argocd/apps/external-secrets-operator.application.yaml` | ~32 `ExternalSecret`, 4 `ClusterSecretStore` (all → AWS Secrets Manager us-east-1) |
| KEDA | `remote/argocd/apps/keda.application.yaml` | `ScaledObject` |
| Strimzi (Kafka) | `remote/argocd/apps/dd-kafka-strimzi.application.yaml` | Kafka CRs |
| Spark Operator | `remote/argocd/apps/dd-spark-operator.application.yaml` | SparkApplication |
| Actions Runner Controller | `remote/argocd/apps/canonical-ci-arc-controller.application.yaml` | runner scale sets |
| cert-manager | Hetzner only (annotations reference `letsencrypt-prod` issuer) | `Certificate`, via ingress annotations |
| metrics-server | `remote/argocd/apps/metrics-server.application.yaml` | (no CRDs) |

And ArgoCD itself is the meta-operator: its `Application` CRD is what makes the
whole `remote/argocd/` tree declarative.

What we have **zero** of:

- Crossplane (no mention anywhere in the repo).
- Custom in-repo CRDs or controllers.

Out-of-cluster resources are split between **Terraform**
(`remote/terraform/cloudflare/r2/`, `remote/terraform/aws/airbyte-s3/`) and
**shell scripts** (kubeadm bootstrap under `remote/hetzner/` and `remote/ec2/`,
image sync, cert renewal).

---

## 3. The four operator-shaped areas, evaluated

These are the places where manual or scripted work looks like it could become an
operator. Verdicts differ per area — this is where the "custom operator?"
question gets its real answer.

### 3.1 Per-tenant scaffolding — the strongest itch, but templating wins

Registering a tenant today (see commit `94601064`, "Register zed-pkg org
tenant") means hand-copying the threefa/daedalus precedent:

- `remote/argocd/projects/<org>.tenant.yaml` — Namespace (PSA labels) +
  ResourceQuota + LimitRange + default-deny NetworkPolicy
- `remote/argocd/projects/<org>.appproject.yaml` — locked-down AppProject
  (`clusterResourceWhitelist: []`, pinned `sourceRepos`/`destinations`)
- `remote/argocd/apps/<org>.applications.yaml` — one Application per app repo
- `.gitmodules` + `SUBMODULES.md` inventory entries

A custom "Tenant operator" (`kind: OrgTenant` → controller stamps out all of the
above) is the textbook demo case for kubebuilder. But look at the numbers: **3-4
tenants, growing by roughly one per registration event, with essentially zero
post-creation lifecycle** (a tenant, once created, is static YAML that ArgoCD
enforces). Operators earn their keep on *ongoing reconciliation*, not one-time
stamping. A controller here would run 24/7 to do work that happens a few times a
year — and its CRD would become one more schema to version.

Better fits, in ascending order of machinery:

1. **Keep the copy-paste, formalize the template.** `_tenant-scaffold.template.yaml`
   and `_template.appproject.yaml` already exist in `remote/argocd/projects/`.
   A 20-line generator script (sed/envsubst) that emits the three files from an
   org name captures 90% of the value at ~0% of the cost.
2. **ArgoCD ApplicationSet.** ArgoCD (which we already run) ships a generator
   CRD purpose-built for exactly this: a git-directory or list generator that
   templates an Application (or app-of-apps that includes the tenant scaffold)
   per entry. Adding a tenant becomes adding one list element. This is
   "operator behavior" — from an operator we already own, with no code written.

**Verdict: no custom operator. Template script now; ApplicationSet if tenant
count grows past ~10 or tenants gain dynamic lifecycle (offboarding, quota
tiers).**

### 3.2 Cert rotation on AWS — a solved problem; adopt, don't build

`remote/ec2/renew-letsencrypt-gateway-cert.sh` plus a systemd timer runs certbot
on the host, `kubectl create secret`s the result, and restarts
`dd-remote-gateway`. This is a hand-rolled cert-manager — and we *already run
cert-manager on Hetzner*, where the same gateway gets TLS via a
`letsencrypt-prod` ClusterIssuer annotation.

This is the purest example of why the operator pattern exists: certificate
renewal is many instances × recurring lifecycle × failure-retry logic — exactly
above the break-even line. And the operator already exists, is battle-tested, and
is already in our stack on another cloud.

**Verdict: clearest concrete win in this doc. Install cert-manager on the AWS
cluster (HTTP-01 or DNS-01 via Cloudflare), delete the certbot/systemd/kubectl
glue. Zero custom code. This also converges the Hetzner and AWS overlays, which
`remote/argocd/clusters/README.md` states as a goal.**

### 3.3 Postgres schemas and migrations (dpm) — deliberately *not* an operator

`remote/libs/pg-defs/` + `dpm.sh` is declarative in spirit (schema.sql as source
of truth, `dpm diff|verify|apply`) but human-triggered by explicit policy:
"never apply automatically, a human reviews."

A migration operator (or a schema-management CRD à la Atlas Operator / CNPG's
managed roles) would remove exactly the safety property we chose. Schema DDL
against a shared multi-tenant database is the category of change where a
reconcile-loop retry is dangerous, not helpful — and the known dpm fixed-point
bug on varchar IN-list CHECK constraints (dpm never converges; CI runs `dpm
verify` as advisory) is a live demonstration: a controller wrapping a
non-convergent differ would loop-apply forever.

Where an operator *could* help later, without touching DDL: **provisioning**
(create database/role/namespace per new app on the shared instance) is
mechanical and idempotent. If in-cluster Postgres ever grows beyond the current
footprint, CloudNativePG is the upstream operator to evaluate — HA, backups,
and role management as CRDs — rather than anything custom.

**Verdict: keep human-in-the-loop by design. Revisit only for the
provisioning (not migration) half, and with CNPG, not custom code.**

### 3.4 External cloud resources — the actual Crossplane question

Today: two small Terraform roots (Cloudflare R2 buckets + custom domains; the
Airbyte S3 bucket + IAM user + Secrets Manager entry) and everything else
static (Cloudflare DNS records for `*.fiducia.cloud`, Supabase projects, the
external RDS-style Postgres, AWS Secrets Manager itself).

Crossplane would let ArgoCD manage these the same way it manages Deployments:
`Bucket`, `Record`, and `SecretsManagerSecret` CRs in `remote/argocd/`, drift
auto-corrected, no tfstate to protect. Longer-term, a `Composition` could even
express "tenant" end-to-end: one claim → namespace scaffold *in* the cluster +
R2 bucket + secrets entry *outside* it, which no amount of Kustomize can do.

Honest cost accounting for our footprint, though:

- Resource count is small (a couple of buckets, one IAM user, DNS records) and
  churn is low. Terraform's operational cost at this scale is near zero.
- Crossplane's fixed cost is not: control plane + AWS provider + Cloudflare
  provider pods, hundreds of installed CRDs, provider version upgrades, and a
  cloud-credentials-in-cluster security surface (a Crossplane provider
  credential is by construction broad — it must be able to create/destroy
  infrastructure).
- Our multi-cloud story is *portability* (same manifests, per-cloud overlays),
  not *cloud-resource sprawl* — the latter is what Crossplane is for.

**Verdict: not now, but this is the one to re-evaluate first. Trigger
conditions: (a) app orgs need self-serve cloud resources (a bucket per tenant,
say) so that claims-in-git beat Terraform PRs; (b) the Terraform surface grows
past a handful of roots; or (c) drift from out-of-band console changes becomes
a real incident source. If adopted, start narrow — one provider, one resource
type (R2 buckets), no Compositions until the plain managed resources prove out.**

---

## 4. Decision summary

| Area | Custom operator? | Do instead |
|---|---|---|
| Tenant scaffolding | No | Generator script from existing `_template` files; ArgoCD ApplicationSet at >~10 tenants |
| AWS gateway TLS | No | **Adopt cert-manager on AWS** (already on Hetzner); retire certbot/systemd glue |
| DB migrations (dpm) | No — by policy | Keep human review; CNPG only if in-cluster Postgres grows, and only for provisioning/HA |
| External cloud resources | No (today) | Keep Terraform; re-evaluate Crossplane on the triggers in §3.4 |

The general principle this cluster already follows, made explicit: **buy
operators, don't build them.** Our leverage comes from composing mature upstream
controllers (ArgoCD, ESO, KEDA, Strimzi, cert-manager) under GitOps. A custom
controller becomes justified only when we own a resource type with real ongoing
lifecycle that no upstream project models — and none of our current pain points
clears that bar. The closest future candidate is a tenant Composition under
Crossplane, and even that waits until tenant count and cloud-resource-per-tenant
needs materialize.

## 5. References

- Operator pattern: https://kubernetes.io/docs/concepts/extend-kubernetes/operator/
- kubebuilder (if we ever do build one): https://book.kubebuilder.io/
- ArgoCD ApplicationSet: https://argo-cd.readthedocs.io/en/stable/operator-manual/applicationset/
- Crossplane: https://docs.crossplane.io/ — providers: upbound AWS, Cloudflare (community)
- cert-manager: https://cert-manager.io/docs/
- CloudNativePG: https://cloudnative-pg.io/
- In-repo context: `remote/argocd/clusters/README.md` (overlay contract),
  `remote/argocd/projects/README.md` (tenant model), `docs/app-deploy-contract.md`,
  `docs/gitops-boundary-audit.md`
