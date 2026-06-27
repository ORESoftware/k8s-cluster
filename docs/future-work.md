# Future Work

## Storage Decision

Config KV and the other coordination primitives are not backed by
Postgres/Supabase. Their source of truth is the owning shard's Raft log plus the
applied state-machine snapshot on each `fiducia-node`.

Use embedded RocksDB per node for production durability:

- `fiducia-node.rs`: Raft log, Raft metadata, snapshots, applied coordination
  state, and watch indexes under `FIDUCIA_NODE_DATA_DIR`.
- `fiducia-interfaces`: Supabase/Postgres schema only for business-plane data:
  orgs, projects, users, RBAC, API keys, mTLS identities, audit, billing, and
  dashboard metadata.
- `fiducia-brain.rs`: brain membership and shard-placement state eventually
  needs its own small replicated brain Raft group; it should not use the
  customer coordination KV as its control-plane database.

## Highest-Value Product Gaps

1. **Auth, RBAC, and audit**
   Owner repos: `fiducia-auth.rs`, `fiducia-interfaces`, `fiducia-admin.rs`,
   `fiducia-edge`.

   Add orgs, projects, per-key permissions, scoped short-lived tokens,
   project-scoped API keys, audit logs, and optional mTLS client certificates.
   API keys stay the customer DX; auth/LB caches introspection so the hot path
   does not call Supabase or Postgres on every request.

2. **SDKs and CLI**
   Owner repos: `fiducia-clients`, `fiducia-cli.rs`, `fiducia-interfaces`.

   Treat TypeScript, Go, Rust, and Python as the first production tier. Generate
   payload types from `fiducia-interfaces`; keep `fiducia-clients/PROTOCOL.md`
   as the method/endpoint contract. Grow `fiducia-cli.rs` from closest-region
   selection into `fiduciactl`: login, project selection, API key lifecycle, KV
   get/put/watch, lock inspect, schedule history, shard health, and support
   bundles.

3. **Admin and observability APIs**
   Owner repos: `fiducia-node.rs`, `fiducia-brain.rs`, `fiducia-admin.rs`,
   `fiducia-telemetry.rs`, `fiducia-backend.rs`.

   Expose lock holders, FIFO wait queues, lease state, leader history, schedule
   run history, shard health, quorum status, per-shard latency, Raft term/index,
   snapshot/compaction lag, and route/redirect counts. Admin UI should be a thin
   authenticated web tier over those APIs.

4. **Transactions and workflows**
   Owner repos: `fiducia-node.rs`, `fiducia-interfaces`, `fiducia-clients`.

   Add atomic multi-key CAS when keys share a shard, batch acquire, lock plus
   config update, semaphore plus fencing token, and a cross-shard workflow story
   for operations that cannot be made single-shard.

5. **Disaster recovery and retention**
   Owner repos: `fiducia-node.rs`, `fiducia-brain.rs`, `fiducia-infra`,
   `fiducia-admin.rs`.

   Add snapshot export/import, restore drills, regional failover, schedule run
   retention, audit retention, watch-history retention, and fencing-token-safe
   restore semantics.

6. **Service discovery depth**
   Owner repos: `fiducia-node.rs`, `fiducia-load-balance.rs`,
   `fiducia-routing.rs`, `fiducia-clients`.

   Add DNS endpoint support, active health checks beyond TTL heartbeat, tags,
   metadata filtering, prepared queries, locality-aware lookup, and load-balancer
   integration.

7. **Compatibility layer**
   Owner repos: new adapter repo or `fiducia-edge`, plus `fiducia-node.rs`.

   Consider an etcd-compatible subset first: KV get/put/delete/watch, leases,
   locks, and elections. A ZooKeeper-style recipe layer can follow if customers
   need migration without rewriting coordination code.

## Submodule Branch Tracking

Git supports `submodule.<name>.branch = .`, meaning "use the same branch name as
the current superproject branch" for submodule remote updates. See the
[Git submodule docs](https://git-scm.com/docs/git-submodule).

That could make feature-branch workflows cleaner than rewriting every
`.gitmodules` entry to `feature/foo`. For release pins, explicit `main`/`dev`
is still clearer.
