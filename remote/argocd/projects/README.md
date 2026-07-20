# Argo AppProjects + tenant scaffolds — enforced app/platform boundary

Groundwork for making the boundary in [docs/app-deploy-contract.md](../../../docs/app-deploy-contract.md)
**enforced** instead of conventional. Everything here is **additive and not yet wired into
any cluster apply** — creating an `AppProject` changes nothing until an `Application`
references it via `spec.project`.

## Files

- `_template.appproject.yaml` — per-app Argo `AppProject`: pins `destinations` to one
  namespace, `sourceRepos` to one repo, `clusterResourceWhitelist: []`. This is the guard
  that fails the sync if an app commits a ClusterRole/CRD/Namespace.
- `_tenant-scaffold.template.yaml` — the Layer-1 tenancy objects the platform owns per app:
  `Namespace` + `ResourceQuota` + `LimitRange` + default-deny `NetworkPolicy`.

## Why both are needed together

An `AppProject` with `clusterResourceWhitelist: []` forbids the app from creating its own
`Namespace`. So the platform must create the namespace first (tenant scaffold), then the app
syncs workload-only manifests into it. Today even the reference app (`3fa-backend`) ships its
own `namespace.yaml` — that must move here before its project can go strict.

## Per-app adoption (safe, incremental — one app at a time)

1. Copy `_tenant-scaffold.template.yaml` → `<app>.tenant.yaml`, fill in `<app>`, apply it
   (or add an Application pointing at it). Namespace now exists, platform-owned.
2. In the **app repo**, delete its `namespace.yaml` and drop `CreateNamespace=true`.
3. Copy `_template.appproject.yaml` → `<app>.appproject.yaml`, fill in `<app>/<org>/<repo>`,
   apply it.
4. Flip that app's `Application`: `spec.project: <app>`, `destination.namespace: <app>`,
   `repoURL` → the app repo, `path: deploy/k8s`.
5. Verify the app syncs green. If it fails on a cluster-scoped resource, that resource
   belongs in Layer 1 — move it, don't widen the whitelist.

## Suggested order

`3fa` (already namespace-isolated) → `athleto` → `quaestor` (billing+web) →
`fiducia` api/web (after node/brain are retired here) → the rest. Thin/deferred orgs
(canonical, claritas) can stay monorepo-managed; give them a project anyway so the guard
still applies.
