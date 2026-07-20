# Deployment

Fiducia uses pull-based GitOps. Component repositories test and publish
artifacts, the monorepo records the approved release, and Argo CD is the only
production reconciler. GitHub Actions never receives Kubernetes or cloud
credentials.

## Placement and desired-state ownership

The production topology has a deliberately asymmetric fourth cluster:

| Plane | Runtime cluster | Desired-state owner | Workloads |
|---|---|---|---|
| Web | ORESoftware `k8s-cluster` | currently `~/codes/ores/k8s-cluster`; target `fiducia-monorepo` | `fiducia-admin`, `fiducia-backend` (the customer app), `fiducia-auth`; shared gateway/observability remain cluster-owned |
| Data | `fiducia-infra` on Hetzner, Civo, Vultr | `fiducia-monorepo` | `fiducia-node`, `fiducia-node-sidecar`, `fiducia-brain`, `fiducia-load-balance`, per-cluster telemetry agent |

The web cluster must never carry the Argo label `fiducia.cloud/plane=data`.
The production ApplicationSet requires that label, plus an explicit provider
allowlist, so it cannot accidentally place Raft members on the web plane.

## Component repositories — CI and images only

Container-producing repositories publish only a commit-SHA tag to GHCR, with
BuildKit provenance and an SBOM. They do not publish `latest`, hold a kubeconfig,
or call Argo CD. The production promotion resolves each reviewed commit tag to
its registry digest, so the running image remains immutable even if a tag is
later moved.

The customer image includes its reviewed static-site fallback. Production pods
do not clone GitHub repositories or compile Rust/Astro at startup.

## Monorepo promotion

The manual `deploy` workflow is a release promotion, not a cluster deployment:

1. The protected `prod` Environment approves a specific `main` commit.
2. The workflow checks out the exact recursive gitlink set using the read-only
   `FIDUCIA_SUBMODULE_TOKEN`.
3. It verifies the four core images exist and resolves their GHCR digests.
4. `tools/gitops-release.mjs` renders the reviewed `fiducia-infra` overlays for
   Hetzner, Civo, and Vultr, rejects Secrets and non-digest core images, and
   updates `gitops/release.json` plus `gitops/data-plane/*/manifests.yaml`.
5. After a second contract/build check, the workflow commits only that desired
   state to `main`. Argo CD observes the commit and reconciles the clusters.

There is no direct `kubectl apply`, rollout loop, Argo API call, or kubeconfig in
Actions. A promotion commit made with `GITHUB_TOKEN` does not start another
workflow, so validation is deliberately completed before the commit is pushed.

## Argo CD bootstrap

`gitops/argocd/production-applicationset.yaml` defines a restricted AppProject
and cluster-generator ApplicationSet. Bootstrap it once on the Argo CD hub after
registering the three provider clusters. Their Argo cluster Secrets require:

```yaml
fiducia.cloud/cluster: "true"
fiducia.cloud/environment: production
fiducia.cloud/plane: data
fiducia.cloud/provider: hetzner # civo or vultr on the other clusters
```

The AppProject omits `Secret`, so TLS keys, image-pull credentials, database
URLs, Supabase credentials, and Fiducia internal trust material remain in the
cluster secret-management plane. Configure repository credentials for the
monorepo in Argo CD if the repository is restored to its intended private
visibility.

Before enabling this ApplicationSet, remove the legacy node/brain/load-balancer
resources from the ORESoftware cluster's `fiducia` Argo Application. That
Application temporarily remains the web-plane owner for admin,
customer/backend, and auth; two Argo Applications must never manage the same
Kubernetes object. The target is for the monorepo to own that desired state as
well. Follow the staged ownership-transfer checklist in
[`k8s-cluster-gitops-todos-2026-07.md`](k8s-cluster-gitops-todos-2026-07.md)
before adding or enabling a web-plane Application.

## Required GitHub controls

- Protect the `prod` Environment with required reviewers and restrict it to
  protected `main`.
- Store only `FIDUCIA_SUBMODULE_TOKEN` there. It needs read access to private
  Fiducia component repositories and no write scopes.
- Keep the deploy job's `contents: write` permission; it is used only to append
  the approved desired-state commit to `main`.
- Require the `ci / contracts` check on `main`. The contract suite validates
  the Argo selector, release bill of materials, manifest hashes, Kustomize
  builds, digest pins, and Secret exclusion.
