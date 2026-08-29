# Ownership-aware namespace migration

Linear: [DEN-2786](https://linear.app/denman/issue/DEN-2786)  
GitHub: [ORESoftware/k8s-cluster#1104](https://github.com/ORESoftware/k8s-cluster/issues/1104)  
Planning surface: [dancing-dragons Project 4](https://github.com/orgs/dancing-dragons/projects/4)

## Decision

The legacy `dd/` family is not replaced globally with `ores/`.

The migration uses stable ownership roots:

- `ores/` is reserved for shared ORESoftware platform and cluster-control assets;
- product and shared-service assets use registered owner roots such as `fiducia-cloud/`, `sonus-auris/`, `zed-pkg/`, and `shared-auth/`;
- exact lowercase GitHub owner slugs are the default namespace IDs, but the checked-in ID remains stable if a GitHub owner is renamed later;
- short aliases such as `fiducia` and `zed` are discovery aids during migration, not approved target roots;
- unknown legacy values remain `unclassified` and may not silently fall into `ores/`.

This avoids laundering product ownership through a new shared prefix and makes least-privilege IAM, tenancy, lifecycle, cost, and incident ownership visible.

## Phase 0 boundary

This change is a read-only governance and evidence layer. It does not:

- copy, rotate, create, or delete a secret;
- change Cloudflare, R2, DNS, IAM, Kubernetes, Argo CD, a database, or a host path;
- rewrite an existing manifest;
- rename a Kubernetes object, image, repository, binary, or package;
- authorize a production rollout.

Provider writes begin only after the inventory is reviewed, every active occurrence has an accountable owner and rollback plan, and a low-risk stateless pilot is selected.

## Contract files

| Path | Purpose |
| --- | --- |
| `catalog/namespaces/owners.json` | Stable namespace IDs, GitHub-owner mappings, aliases, owner type, and approved environments |
| `catalog/namespaces/owner-registry.schema.json` | Editor and machine schema for the owner registry |
| `catalog/namespaces/migration-rules.json` | Ordered classification rules and fail-closed fallbacks |
| `catalog/namespaces/migration-rules.schema.json` | Editor and machine schema for the rule set |
| `tools/namespace_migration.py` | Read-only validator, repository inventory, and pull-request ratchet |
| `tools/test_namespace_migration.py` | Unit and adversarial tests |
| `.github/workflows/namespace-migration-contract.yml` | Exact-head validation and inventory evidence |

The JSON format intentionally follows the repository's existing GitOps catalog conventions and keeps validation on the Python standard library. It does not introduce a YAML parser or a second ownership-discovery mechanism.

## Naming contract by system

A slash hierarchy is correct for some provider paths, but not for every system.

### Cloud secret paths

Use:

```text
<owner>/<environment>/<workload>/<secret-object>
```

Examples:

```text
ores/prod/cluster/argocd-github-app
fiducia-cloud/prod/fiducia-node/runtime
shared-auth/prod/shared-auth-server/signing
sonus-auris/prod/sonus-api/shared-auth-client
zed-pkg/dev/registry/github-app
```

`ores/` is not a default. A rule that maps a product-owned slash namespace into `ores/` fails validation.

Shared services retain their own root. A Sonus Auris client of Shared Auth receives an explicit cross-owner grant; the Shared Auth signing secret does not become a `sonus-auris/` secret.

### Kubernetes labels and annotations

Use fixed DNS-qualified keys controlled by the platform, for example:

```text
platform.oresoftware.com/thread-id
platform.oresoftware.com/user-id
platform.oresoftware.com/owner
platform.oresoftware.com/environment
secrets.oresoftware.com/fiducia-workload
```

Owner identity is a label value, such as:

```text
platform.oresoftware.com/owner=fiducia-cloud
```

Do not create a label-key prefix per GitHub organization. Selectors may be immutable, so the later migration must dual-emit keys, update readers, roll or recreate workloads, and only then remove legacy keys.

### Kubernetes Namespace and Argo CD tenancy

Kubernetes Namespace names cannot contain `/`. Use:

```text
<owner>[-<environment>]
```

Examples include `shared-auth`, `fiducia-cloud-prod`, and `sonus-auris-dev`.

Each product boundary should have ownership labels, a constrained Argo CD `AppProject`, namespace-scoped RBAC, default-deny NetworkPolicy, source and destination allowlists, and quotas where appropriate.

The merged GitOps composition catalog remains authoritative for exact repository and gitlink identity. Its `spec.owner` values are discovered by this contract and compared with the owner registry; this migration does not replace that catalog.

### Shared-node paths

Keep one platform-controlled host root:

```text
/opt/ores
/var/lib/ores/<service>
/srv/ores/repos/<owner>/<repo>
```

Do not create unrelated top-level `/opt/fiducia-cloud` or `/opt/sonus-auris` roots on shared nodes. Stateful paths such as NATS or JetStream require service-specific stop, backup, integrity, restart, and rollback evidence.

### Source and generated packages

Go modules must match the real repository authority. Java packages use a deliberate reverse-DNS authority. Generated `dd_pg_defs` and `dd.pgdefs` identifiers are changed at the generator and all known dependents are certified; they are not hand-edited and are not assigned an invented blanket `/ores/` path.

Hyphenated `dd-*` Kubernetes object, image, application, binary, and repository names are a separate follow-up. This contract only governs the namespace forms it can identify safely.

## Commands

Validate the catalog and summarize current debt:

```bash
python3 tools/namespace_migration.py check --root . --format text
```

Emit the complete machine-readable occurrence inventory:

```bash
python3 tools/namespace_migration.py inventory \
  --root . \
  --format json \
  > namespace-inventory.json
```

Include governance examples when testing the classifier itself:

```bash
python3 tools/namespace_migration.py inventory \
  --root . \
  --include-governance \
  --format json
```

Reject newly added legacy references between exact commits:

```bash
python3 tools/namespace_migration.py ratchet \
  --root . \
  --base-ref <base-commit> \
  --head-ref <head-commit> \
  --format text
```

Run unit and adversarial tests:

```bash
python3 -m py_compile \
  tools/namespace_migration.py \
  tools/test_namespace_migration.py
python3 tools/test_namespace_migration.py
```

## Classifier status

- `classified`: the owner and target grammar are approved. Per-occurrence environment, workload, consumers, and rollout evidence may still be required.
- `review-required`: ownership or root is known, but the final target or migration procedure needs human and agent review.
- `unclassified`: no target is permitted. The reference remains quarantined until an owner, consumers, verification, and rollback are recorded.

Rules are ordered by explicit priority. Specific product or platform rules therefore win over generic `dd/remote-dev/` and `dd/` fallbacks.

The generated inventory records:

```text
path, line, column, scope, system, reference, rule, owner, status, target preview
```

Scopes are `active`, `documentation`, `test`, and `governance`. Active unclassified references become hard failures when `--strict-unclassified` is enabled after the initial inventory is reviewed.

## Pull-request ratchet

The repository contains substantial pre-existing legacy debt. A raw hard-zero grep would either block every unrelated pull request or encourage a broad unsafe replacement.

The ratchet instead compares the exact pull-request base and head commits and rejects only newly added legacy references. Existing lines may be migrated incrementally without allowing the debt to grow.

Governance files may contain examples of legacy names. A non-governance line can use the marker below only for a narrow, reviewed compatibility exception:

```text
namespace-migration: allow-legacy
```

The marker is intentionally line-scoped. It is not an exemption for an entire file or directory and should include nearby rationale and a removal issue.

## Migration manifest generated from the inventory

The CI inventory artifact is the source for the reviewed migration manifest. Before provider writes, every distinct active reference must be enriched with:

```text
system,current,target,owner,environment,workload,consumers,migration_mode,rollback,verification,status
```

The migration implementation must reject:

- non-platform assets mapped to `ores/` without an approved exception;
- unknown owners or target roots;
- two legacy values collapsing onto one exact target;
- cross-owner reads without an explicit consumer grant;
- invalid metadata authorities or host roots;
- deletion before verification and the agreed grace period.

## Rollout sequence after Phase 0

1. Review the CI inventory and register any real GitHub owner that is still missing.
2. Resolve all active `unclassified` occurrences and enable `--strict-unclassified`.
3. Prepare owner- and workload-scoped IAM while keeping old paths readable during the bounded grace period.
4. Pilot one stateless, non-authentication, non-stateful workload.
5. Migrate cloud paths one owner at a time with copy or dual-write, metadata comparison, consumer flip, health verification, and rollback.
6. Migrate Kubernetes metadata with dual emission and selector-aware rollout.
7. Move GitOps source layout and tenancy in small owner-scoped batches without changing application identity unnecessarily.
8. Migrate shared-node paths; handle each stateful service separately.
9. Change real code and generator authorities and certify external dependents.
10. Delete legacy paths, narrow IAM, remove compatibility links and dual labels, and switch the CI policy to hard zero.

Do not use Shared Auth signing/session state, Fiducia encryption/bootstrap/recovery, NATS/JetStream data, or another circular recovery dependency as the first pilot.

## Review and coordination evidence

The workflow publishes three artifacts from the exact pull-request head:

- `namespace-contract-report.json` — catalog validity and summary counts;
- `namespace-inventory.json` — complete classified occurrence inventory;
- `namespace-ratchet.json` — exact base/head new-reference result.

These artifacts are intended for independent ChatGPT, Claude, owner, and security review. Agent agreement is planning evidence, not authorization to mutate providers or delete old data.
