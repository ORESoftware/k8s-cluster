# Submodule-backed GitOps application fleet

**Status:** incremental implementation plan and read-only pilot  
**Linear:** DEN-2724, DEN-2725, DEN-630  
**GitHub:** ORESoftware/k8s-cluster#1097

## Terminology

“App of Apps” is reserved for the specific Argo CD topology in which one
`Application` renders child `Application` resources. It does not mean that one
Argo CD installation is nested inside another.

The broader ORESoftware design is an **ApplicationSet-managed application
fleet** backed by a **GitOps composition catalog**:

```text
k8s-cluster gitlink inventory
        │ exact child commit
        ▼
catalog/gitops/apps/<application>.json
        │ validated direct source + placement policy
        ▼
ApplicationSet
        │ generated, independently reconcilable Application
        ▼
application repository at the exact gitlink commit
```

A thin bootstrap Application may eventually own AppProjects and
ApplicationSets. It must not turn unrelated products into one shared health,
prune, and rollback boundary.

## Why submodules remain

A `k8s-cluster` commit is a reviewable bill of materials: every mode-160000
gitlink identifies an exact child repository commit. Submodules are useful for
source composition, local development, integration testing, and promotion
review.

They are not the Argo render mechanism. Current policy keeps repo-server
submodule initialization disabled after the prior submodule-init incident.
Application manifests therefore point directly at each application repository.
The composition validator proves that the direct `targetRevision` equals the
superproject gitlink SHA.

This separates responsibilities cleanly:

- `.gitmodules` declares repository identity;
- the superproject index declares the exact child commit;
- `catalog/gitops/apps/*.json` declares deployment intent and placement;
- ApplicationSet generates independent Argo CD Applications;
- Argo CD reconciles application-owned namespace-scoped manifests;
- AppProject and parent-owned tenancy resources enforce boundaries.

## Catalog boundaries

The existing DEN-630 `catalog/applications.json` remains the observed inventory
of tracked Argo CD Application declarations. The DEN-2724 composition catalog
does not replace or duplicate it. Instead, it records the desired invariant
between a gitlink and a direct Argo source.

Each record is schema constrained and contains:

- owner and application identity;
- submodule path, upstream repository, and exact indexed revision;
- direct Argo repository, exact target revision, manifest path, and renderer;
- AppProject, namespace, destination server, and sync posture;
- migration phase and the static Application retained for rollback.

The first record is intentionally `pilot-inert`: it cannot prune, self-heal, or
sync automatically.

## Validator split

### `k8s-cluster`

`tools/gitops_composition.py` is the reference policy implementation. It owns
the repository-specific path, tenancy, Argo, migration, and `*-infra` rules. It
requires no Kubernetes or production credentials.

### Zed

`zed-gitops validate` is the portable CLI surface. It consumes the same JSON
contract and uses generic Git/submodule evidence. The executable is kept
separate from the core package-manager command graph so Kubernetes-specific
policy does not leak into ordinary package installation.

Future factoring can move canonical repository identity, `.gitmodules`,
gitlink, dirty-state, and duplicate-ownership primitives into reusable
`zed-pkg` libraries while the policy remains versioned here.

## Delivery sequence

1. Land the schema, exact-pin validator, deterministic preview, and inert
   ApplicationSet prototype.
2. Run the validator in required CI without initializing submodules.
3. Prove the pilot app's exact child commit renders successfully from its direct
   repository.
4. Verify repository credentials, AppProject, namespace, health, observability,
   resource ownership, deletion behavior, and rollback.
5. Introduce the generated `catalog-pilot-*` Application without removing the
   static Application.
6. Compare rendered and live resource ownership.
7. In a separate activation PR, remove the static declaration and switch the
   generated Application to the canonical name.
8. Add records one ownership unit at a time.

## Non-goals

- enabling Argo CD to render through nested submodules;
- replacing Git submodules with a package manager;
- deploying or mutating a cluster from the validator;
- putting credentials or secrets in the catalog;
- automatically enabling prune during migration;
- classifying `*-infra` repositories as applications.
