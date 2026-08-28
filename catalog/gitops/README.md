# GitOps composition catalog

This directory is the DEN-2724 companion to the existing DEN-630
`catalog/applications.json` inventory.

The two catalogs answer different questions:

| Catalog | Question |
| --- | --- |
| `catalog/applications.json` | What Argo CD `Application` declarations are currently tracked? |
| `catalog/gitops/apps/*.json` | Which exact application gitlink pin is authorized to become which direct Argo CD source? |

Git submodules remain the repository-composition and provenance mechanism. They
are **not** an Argo CD render root. The repo-server is intentionally allowed to
leave submodules uninitialized. Every record therefore pairs:

1. an exact mode-160000 gitlink in `k8s-cluster`;
2. its canonical upstream repository from `.gitmodules`; and
3. a direct Argo CD source using that same repository and exact commit.

This preserves a reviewable bill of materials without requiring Argo CD to
render files through nested submodules.

## Validate

```console
python3 tools/gitops_composition.py check --root .
python3 tools/gitops_composition.py check --root . --format json
```

The check uses only `.gitmodules`, the superproject index, catalog JSON, and
parent-owned static manifests. It does not initialize or execute a child
repository.

## Preview

```console
python3 tools/gitops_composition.py render --root . \
  > /tmp/gitops-application-preview.json
```

Preview output is deterministic Kubernetes JSON. It is evidence only and is not
applied by the tool.

## Pilot

`dd-fabrication-server` is the first inert record. Its catalog source revision
equals the exact `remote/deployments/fabrication-server-rs` gitlink. The
ApplicationSet prototype under `remote/argocd/application-sets/` is deliberately
not included in any active Kustomization or bootstrap root and generates a
collision-free `catalog-pilot-*` name.

Activation requires a separate reviewed change that proves AppProject,
namespace, repository credentials, render output, resource ownership, rollback,
and deletion behavior before replacing the existing static Application.

## Policy highlights

- app inventory paths are under `remote/deployments/`;
- `*-infra` repositories cannot be app records;
- source and inventory repositories must canonicalize to the same GitHub repo;
- source revisions are exact lowercase 40-hex commits;
- source revision, catalog inventory revision, and indexed gitlink must match;
- Argo renders the direct upstream repository, never a path in `k8s-cluster`;
- `default` AppProject and destination namespace are rejected;
- `pilot-inert` records cannot enable automated sync, prune, or self-heal;
- unknown fields fail in strict mode.

The planned external Zed command consumes the same contract as
`zed-gitops validate`, with human, JSON, and SARIF output.
