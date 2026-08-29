# Crossplane and Kubernetes Operators — Where They Fit This Cluster

Status: analysis + decision doc, with scaffolding landed for the two conclusions that
survived scrutiny. Nothing in this doc is wired into a live cluster.

The question is whether we would benefit from **custom** operators, and whether Crossplane
belongs here. Short answer:

> We already run a stack of third-party operators and get real leverage from them. We should
> **not write a custom operator** — every workload that looks operator-shaped here is
> better served by templating or by an operator that already exists. The one clear win is
> **finishing the cert-manager story**, which is less about adding an operator than about
> the fact that our most security-critical controller is installed by a shell script and
> is invisible to Git. Crossplane is a legitimate future option for out-of-cluster
> resources, but at our footprint Terraform is cheaper to keep.

Two things were built while working this through, both inert:

- [`scripts/new-tenant.sh`](../scripts/new-tenant.sh) — generates the per-org tenant
  scaffold from the existing templates (§3.1).
- [`remote/argocd/cert-manager/`](../remote/argocd/cert-manager/) + an inert
  [`apps/cert-manager.application.yaml`](../remote/argocd/apps/cert-manager.application.yaml)
  — cert-manager as GitOps, plus the DNS-01 path that makes it work on AWS (§3.2).

---

## 1. Concepts, briefly

### The operator pattern

Kubernetes is a reconciliation machine: you declare desired state, controllers notice
drift between desired and actual, and act to converge. An **operator** extends that
machine to things Kubernetes doesn't natively understand, via two halves:

1. **CRDs** — new API types. `ExternalSecret` and `ScaledObject` aren't built in; the
   operator's CRDs teach the API server to store and validate them.
2. **A controller** — a pod that watches those resources and reconciles the world to match
   them, forever, with retries and status reporting.

The value is *encoded operational knowledge*: instead of a human or a cron job knowing
"when the cert is 30 days from expiry, renew it and bounce the gateway," a controller
knows it and does it on a loop with no human in the path.

The cost matters just as much. A **custom** operator is a production service you now own:
a framework (kubebuilder, Operator SDK, kopf), RBAC, CRD version migrations, testing
against API-server upgrades, and on-call understanding. And a controller with a bug
doesn't fail once like a script — it reconciles its bug continuously. Break-even is
roughly **many instances × frequent lifecycle events × logic too dynamic for templates**.
Below that line, templating wins.

### Crossplane

Crossplane is a pre-built family of operators that reconcile **external cloud resources** —
buckets, DNS records, managed databases, IAM users — as Kubernetes custom resources.
Roughly "Terraform as a control loop":

| | Terraform | Crossplane |
|---|---|---|
| Model | Human-triggered apply of a plan | Continuous reconciliation; drift auto-corrected |
| State | A `.tfstate` file to protect and share | The Kubernetes API server *is* the state store |
| GitOps fit | Needs a wrapper (Atlantis, a CI job) | Native — ArgoCD syncs a `Bucket` like any manifest |
| Composition | Modules | `Composition` + XRDs: one `TenantDatabase` claim expands to instance + secret + firewall rule |
| Cost | Near-zero runtime footprint | Control plane + a provider pod per cloud, hundreds of CRDs, provider upgrade churn |

Crossplane shines when a platform team wants app teams to self-serve cloud resources by
committing a small claim, without touching Terraform or a cloud console. It is overkill
when one person manages a handful of static resources.

---

## 2. What we already run

We are already operator-heavy — all third-party, all installed as ArgoCD Applications
pointing at upstream Helm charts:

| Operator | Declared in | What we use |
|---|---|---|
| ArgoCD | bootstrap | the `Application` CRD — the meta-operator the whole `remote/argocd/` tree rides on |
| External Secrets | `apps/external-secrets-operator.application.yaml` | ~32 `ExternalSecret`, 4 `ClusterSecretStore` → AWS Secrets Manager |
| KEDA | `apps/keda.application.yaml` | `ScaledObject` |
| Strimzi | `apps/dd-kafka-strimzi.application.yaml` | Kafka CRs |
| Spark Operator | `apps/dd-spark-operator.application.yaml` | `SparkApplication` |
| Actions Runner Controller | `apps/canonical-ci-arc-controller.application.yaml` | runner scale sets |
| Stakater Reloader | `observability/reloader.deployment.yaml` | opt-in config/secret-triggered restarts |
| cert-manager | **nothing — a shell script** | `Certificate` via ingress annotations |

What we have zero of: **Crossplane**, and **any custom CRD or controller authored here**.

Out-of-cluster resources are split between Terraform (`remote/terraform/cloudflare/r2/`,
`remote/terraform/aws/airbyte-s3/`) and shell scripts (kubeadm bootstrap under
`remote/hetzner/` and `remote/ec2/`, image sync, cert renewal).

---

## 3. The four operator-shaped areas

### 3.1 Per-tenant scaffolding — the strongest itch, and templating still wins

Registering an org today means hand-copying three files per the threefa/daedalus/zed
precedent: a tenant scaffold (Namespace + ResourceQuota + LimitRange + default-deny
NetworkPolicy), an AppProject, and one Application per app repo.

A `kind: OrgTenant` CRD whose controller stamps all of that out is the textbook kubebuilder
demo. It is still the wrong call here, for two reasons.

**The volume isn't there.** Three tenant scaffolds and four AppProjects exist, growing by
roughly one per registration event, with essentially no post-creation lifecycle — once
created, a tenant is static YAML that ArgoCD already enforces. Operators earn their keep on
ongoing reconciliation, not one-time stamping. A controller would run continuously to do
work that happens a few times a year, and its CRD would be one more schema to version.

**The interesting part isn't templatable.** Generating the zed scaffold from the templates
and diffing it against what was actually committed is instructive — the Applications come
out semantically identical, but the committed tenant file differs:

```
ResourceQuota/zed-quota   template: requests.cpu 2, requests.memory 4Gi, limits.memory 8Gi
                          committed: requests.cpu 1, requests.memory 1Gi, limits.memory 4Gi
```

Someone looked at what zed actually needed and tightened it. That is the judgment an
operator would have to either encode (it can't — it's per-tenant) or flatten (which is
how you get every tenant holding a template's default quota). The correct tool generates
a *starting point* a human then tunes, which is exactly what a script does and exactly what
a reconciling controller fights you on.

**What was built:** [`scripts/new-tenant.sh`](../scripts/new-tenant.sh) renders the three
files from `remote/argocd/projects/_*.template.yaml`, refuses to overwrite without
`--force`, validates DNS-1123 names up front, and prints the adoption checklist. Verified
by regenerating the zed registration and diffing against the committed files.

```bash
scripts/new-tenant.sh zed \
  api-server=https://github.com/zed-pkg/zed-api-server.rs.git \
  web-server=https://github.com/zed-pkg/zed-web-server.rs.git
```

**If the volume ever does arrive**, the next step is still not a custom operator — it's an
ArgoCD **ApplicationSet**, a generator CRD purpose-built for this from an operator we
already run. There is already a worked example in
[`remote/argocd/gateway-edge/applicationset.yaml`](../remote/argocd/gateway-edge/applicationset.yaml)
using a cluster generator. Reach for it past ~10 tenants, or when tenants gain real
lifecycle (offboarding, quota tiers).

### 3.2 TLS — the actual win, and it is not "add an operator"

This is where the investigation changed the answer. The problem isn't that we lack
cert-manager; it's that **cert-manager is the one component in the stack that Git does not
describe**. It is installed imperatively by
[`remote/hetzner/ingress-tls.sh`](../remote/hetzner/ingress-tls.sh), which
`kubectl apply --server-side`s the upstream release manifest and applies three
ClusterIssuers from a heredoc. On AWS there is no cert-manager at all — TLS comes from
certbot on the EC2 host plus a systemd timer that runs `kubectl create secret tls` and
`rollout restart`.

So the controller securing every public hostname is invisible to ArgoCD, drifts silently,
and requires re-running a script by hand to rebuild a cluster. The cost is not theoretical:

- The AWS renewal hook broke once because the certbot lineage name was hard-coded to a
  stale EC2 IP (`54.91.17.58`). It now reads `$RENEWED_LINEAGE`, but a certificate bound
  to an IP that can change is a recurring failure, not a fixed bug.
- A failed timer run is silent until the cert actually expires. cert-manager retries across
  the entire 30-day `renewBefore` window and exposes `Certificate` status to alert on.

**The AWS part is genuinely harder than "install cert-manager", which is worth being
precise about.** HTTP-01 cannot work there: cert-manager's HTTP-01 solver must answer on
port 80 at the challenged name, and the gateway pod already owns hostPort 80 and
308-redirects it to :443, with no ingress controller to hand the challenge to. And the
current cert is a Let's Encrypt **IP address** certificate, for which ACME permits only
HTTP-01/TLS-ALPN-01 — never DNS-01. So the migration has a hard prerequisite: **give the
AWS gateway a real DNS name under `fiducia.cloud`**, then solve via Cloudflare DNS-01,
which needs no open port and no ingress controller. That prerequisite is also the durable
fix for the stale-IP breakage.

One more piece falls out for free: rotating the secret isn't enough, because the kubelet
refreshes the mounted files but nginx keeps serving the old cert from memory. Reloader is
already deployed cluster-wide with `--auto-reload-all=false`, so a single opt-in annotation
on the gateway Deployment closes the loop. Another operator we already own.

**What was built:** [`remote/argocd/cert-manager/`](../remote/argocd/cert-manager/) — the
chart Application pinned to v1.16.2 (the exact version the script installs, so adoption is
a takeover rather than an upgrade), ClusterIssuers verified to be an exact mirror of the
live ones, and an opt-in `dns01-cloudflare/` path for AWS. The README there carries the
two-phase migration order and its hazards (field-manager conflicts on takeover; two
writers to the same secret; why CRD pruning is disabled). **Zero custom code.**

### 3.3 Postgres migrations (dpm) — deliberately not an operator

`remote/libs/pg-defs/` + `dpm.sh` is declarative in spirit but human-triggered by explicit
policy: never apply automatically, a human reviews.

A migration operator would remove exactly the safety property we chose. DDL against a
shared multi-tenant database is the category where a reconcile-loop retry is dangerous
rather than helpful — and we have a live demonstration: the known dpm fixed-point bug on
varchar IN-list CHECK constraints never converges, which is why CI runs `dpm verify` as
advisory. A controller wrapping a non-convergent differ would loop-apply forever.

The *provisioning* half (create database/role per new app) is mechanical and idempotent and
could be automated later — but with CloudNativePG, not custom code, and only if in-cluster
Postgres grows past its current footprint.

### 3.4 External cloud resources — the real Crossplane question

Today: two small Terraform roots (Cloudflare R2 buckets and custom domains; the Airbyte S3
bucket plus IAM user plus its Secrets Manager entry), with everything else static —
Cloudflare DNS for `*.fiducia.cloud`, Supabase projects, the external RDS-style Postgres.

Crossplane would let ArgoCD manage these like any other manifest, with drift auto-corrected
and no tfstate to protect. Longer term a `Composition` could express a tenant end-to-end:
one claim producing the in-cluster namespace scaffold *and* an R2 bucket *and* a secrets
entry — something no amount of Kustomize can do, and the one capability that would
genuinely beat what we have.

Honest accounting at our footprint, though: resource count is small and churn is low, so
Terraform's operational cost is near zero, while Crossplane's fixed cost is not — a control
plane plus provider pods, hundreds of installed CRDs, provider version upgrades, and a
broad cloud credential living in the cluster (a Crossplane provider credential must by
construction be able to create and destroy infrastructure). Our multi-cloud story is
*portability* — same manifests, per-cloud overlays — not cloud-resource sprawl, and the
latter is what Crossplane is for.

**Re-evaluate when:** app orgs need self-serve cloud resources (a bucket per tenant) so
claims-in-git beat Terraform PRs; the Terraform surface grows past a handful of roots; or
out-of-band console changes become a real incident source. If adopted, start narrow — one
provider, one resource type (R2 buckets), no Compositions until plain managed resources
prove out.

---

## 4. Decision summary

| Area | Custom operator? | Do instead | Status |
|---|---|---|---|
| Tenant scaffolding | No | Generator script; ApplicationSet past ~10 tenants | [`scripts/new-tenant.sh`](../scripts/new-tenant.sh) landed, inert |
| TLS / cert-manager | No | Put the existing operator under GitOps; DNS-01 for AWS | [`remote/argocd/cert-manager/`](../remote/argocd/cert-manager/) landed, inert |
| DB migrations (dpm) | No — by policy | Keep human review; CNPG only for provisioning/HA, later | no change |
| External cloud resources | No (today) | Keep Terraform; revisit Crossplane on the §3.4 triggers | no change |

The principle this cluster already follows, made explicit: **buy operators, don't build
them.** Our leverage comes from composing mature upstream controllers under GitOps. Writing
one becomes justified only when we own a resource type with real ongoing lifecycle that no
upstream project models — and nothing here clears that bar. The nearest future candidate is
a tenant Composition under Crossplane, and that waits on tenant count and per-tenant cloud
resources actually materializing.

Worth noting what the exercise actually surfaced: the most valuable finding wasn't a
missing operator, it was an operator we already depend on that GitOps doesn't cover. That
is the more common failure mode than "we should have written a controller."

## 5. References

- Operator pattern — https://kubernetes.io/docs/concepts/extend-kubernetes/operator/
- kubebuilder — https://book.kubebuilder.io/
- ArgoCD ApplicationSet — https://argo-cd.readthedocs.io/en/stable/operator-manual/applicationset/
- Crossplane — https://docs.crossplane.io/
- cert-manager, incl. ACME DNS-01 — https://cert-manager.io/docs/configuration/acme/dns01/
- CloudNativePG — https://cloudnative-pg.io/
- In-repo: [`remote/argocd/clusters/README.md`](../remote/argocd/clusters/README.md),
  [`remote/argocd/projects/README.md`](../remote/argocd/projects/README.md),
  [`remote/argocd/gateway-edge/README.md`](../remote/argocd/gateway-edge/README.md),
  [`docs/app-deploy-contract.md`](app-deploy-contract.md),
  [`docs/gitops-boundary-audit.md`](gitops-boundary-audit.md)
