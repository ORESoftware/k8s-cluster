# PostgreSQL gRPC contract reconciler audit — 2026-08-23

## Outcome

The audit converted the service into a report-only reconciler with independently
compiled desired-SQL, protobuf, and ORM witnesses. It does not expose generic
CRUD, apply DDL, or activate a fleet workload. The fleet registration is an
inert, exact-commit pilot and cannot start a Pod as committed.

| Boundary | Reviewed change | Immutable pin |
| --- | --- | --- |
| Shared SQL, JSON Schema, and generated ORM bindings | `ORESoftware/k8s-libs-and-shared-defs#49` | `a31ebf9e72aba9901fe63236413527a02a8dbc38` |
| gRPC reconciliation server | `ORESoftware/grpc-pg-general-connect-server#5` | `37ac95d7be22729bac4a46be99b56a8517abea44` |
| Fleet composition | this change | both pins above |

## Material findings remediated upstream

1. The fleet had no immutable inventory or Argo source registration for the
   server.
2. Ledger JSON was decoded but its schema was not compiled and enforced at the
   server boundary.
3. Generated ORM modules were not compiled together in CI. The first complete
   pass found a GORM field/method collision, an Ent nullable-JSON generation
   defect, and a Bun toolchain mismatch.
4. Qualified PostgreSQL types such as `extensions.vector(1536)` and custom
   `msgint.source_kind` values were losing identity in generated metadata.
5. Stored generated columns were not represented consistently as read-only
   across GORM, Bun, and XORM.
6. The prior Go patch level had a reachable standard-library vulnerability in
   the server call graph. CI, the module, and the build image now use Go 1.26.7
   and run `govulncheck`.
7. The deployment selected a ServiceAccount that did not exist. The audited
   package now declares it with token automount disabled.

## Consistency and formal-method boundary

- The closed Draft 2020-12 ledger schema is compiled in Go and rejects unknown
  fields, over-size input, trailing JSON values, unsafe identifiers, malformed
  digests, invalid generated/read-only combinations, and non-contiguous primary
  key positions.
- GORM, Bun, and XORM are independent voting witnesses over all 204 canonical
  tables. Their normalized projections must match the desired SQL ledger exactly
  for database alias, schema, table, column, physical type, nullability, primary
  key order, generated state, and read-only state.
- Ent remains compiled as a non-voting witness because 34 canonical tables have
  nonstandard or composite primary keys that Ent cannot model without changing
  the database contract.
- Reconciliation requires three lineages (desired SQL, protobuf descriptor, and
  generated bindings), one vote per lineage, and distinct artifact digests.
- The bidirectional stream is governed by a pure transition function. Exhaustive
  bounded trace tests prove that terminal states are absorbing, observation
  cannot precede selection, and at most one report follows each observation.

## Activation gates

The Application intentionally has no automated sync. Even a manual sync renders
zero replicas and an all-zero image digest. Do not activate it until all gates
below have independent review evidence:

At audit time, each upstream GitHub Actions job concluded before its first step
and produced no job log. Local checks are green, but the fleet pin records this
as `local-reproducible-upstream-ci-not-started`; it must not be promoted as
upstream-CI-verified evidence.

1. Merge both upstream PRs and repin this repository to the resulting reviewed
   commits if the merge commits differ.
2. Build, sign, attest, scan, and pin an immutable image digest; remove the
   all-zero sentinel only in that promotion change.
3. Publish an immutable ledger bundle containing desired SQL plus GORM, Bun, and
   XORM ledgers. The current bundle is about 1.8 MiB, so use an image layer,
   object-store artifact, or split projection rather than one ConfigMap.
4. Provision the ExternalSecret inputs and database users. Each database user
   must be least-privilege and forced read-only (including
   `default_transaction_read_only=on`).
5. Prove TLS/mTLS through the pod or service-mesh path before bearer credentials
   traverse the transport. Reconcile the health-probe design with that encrypted
   path.
6. Apply and review `grpc-pg-contracts.appproject.yaml`, then verify its exact
   source, destination, and namespace-resource allowlist.
7. Require exact-head CI, semantic review, security review, and a one-replica
   canary whose first reconciliation remains report-only and balanced.
