# ORES web-plane GitOps cutover TODOs

The target architecture is for `fiducia-monorepo` to own all Fiducia
Kubernetes desired state. The ORESoftware cluster remains the runtime home for
the web plane, but `~/codes/ores/k8s-cluster` should eventually own only shared
cluster facilities such as the gateway, observability, and secret delivery.

This cutover is intentionally deferred. On 2026-07-18 the ORES checkout had an
active writer and its existing `fiducia` Argo CD Application still owned both
web- and data-plane objects. Adding a second reconciler now would create shared
resource ownership and could cause an outage.

## What is ready

- `fiducia-admin.rs`, `fiducia-auth.rs`, and `fiducia-customer.rs` publish
  commit-SHA images to GHCR with maximal provenance and SBOMs:
  `fiducia-admin`, `fiducia-auth`, and `fiducia-backend`.
- The customer image already embeds the reviewed marketing fallback; no source
  checkout or Astro build is needed in a production pod.
- The monorepo promotion flow resolves data-plane images to immutable digests,
  records a release bill of materials, excludes Secrets, and lets Argo CD pull
  the result.
- The production AppProject and data-plane ApplicationSet are isolated from the
  ORES web cluster by the `fiducia.cloud/plane=data` selector.

## Cutover checklist

- [ ] Agree on a maintenance window and obtain explicit write ownership for
  `~/codes/ores/k8s-cluster`. Pause its `fiducia` Application auto-sync before
  changing ownership.
- [ ] Extend `gitops/release.json` with the exact admin, auth, and customer
  gitlinks plus resolved `sha256` image digests. Never promote a mutable tag.
- [ ] Add a rendered `gitops/web-plane/ores/` Kustomize target containing the
  admin, auth, and customer Deployments, Services, NetworkPolicies, HPA, and
  PDBs. Keep the existing stable object names and ports so gateway routing does
  not need an outage-producing rewrite.
- [ ] Replace every in-pod `git clone`, package install, Rust compile, and Astro
  build with the three digest-pinned GHCR images. Run all schema migration work
  as a separately reviewed, idempotent release gate; application startup must
  not own migrations.
- [ ] Keep Kubernetes Secrets out of this repository. Preserve the ORES
  cluster's External Secrets or equivalent out-of-band delivery for database,
  Supabase, CSRF, JWT, internal-trust, TLS, and image-pull material.
- [ ] Add a dedicated `fiducia-production-web-plane` Argo CD Application. Use an
  explicit ORES destination or a cluster selector requiring
  `fiducia.cloud/plane=web`; never broaden the data-plane selector. Reuse the
  restricted project and continue denying `Secret` management.
- [ ] Add contract tests that require all three web images to be digest-pinned,
  reject Secrets and runtime source builds, verify Kustomize output, and bind
  the web Application to exactly one cluster.
- [ ] Extend the protected `deploy` promotion to render and validate web-plane
  state in the same reviewed release commit as the provider data plane. GitHub
  Actions must remain credentialless with respect to Kubernetes and Argo CD.
- [ ] Perform the ownership transfer in order: pause the legacy Application,
  remove its Fiducia web resources from the ORES source path, sync that reduced
  Application, enable the monorepo web Application, and then restore automated
  prune/self-heal. Never leave both Applications reconciling the same object.
- [ ] Remove the legacy ORES node, sidecar, brain, and load-balancer resources
  only after the Hetzner, Civo, and Vultr Applications are all Healthy and
  Synced. This is a separate safety gate from the web-plane transfer.
- [ ] Verify admin and customer login, Supabase session refresh, CSRF rejection,
  auth JWKS/introspection, database readiness, gateway routes, HPA/PDB behavior,
  NetworkPolicy egress, OTLP export, and rollback to the previous release bill
  of materials.

## Completion criteria

The cutover is complete when one monorepo release commit identifies every
Fiducia production image and manifest, Argo CD reports the ORES web Application
and all three provider data Applications Healthy/Synced, and the ORES cluster
repository contains no independently maintained Fiducia workload manifests.
