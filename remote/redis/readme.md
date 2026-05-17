# Redis cache

`dd-redis-cache` is the cluster-local Redis cache declared in
`remote/argocd/dd-next-runtime`. It is currently configured as ephemeral runtime cache only:

- no append-only file persistence
- no RDB snapshots
- no PVC or host data volume
- bounded memory with `allkeys-lru` eviction
- command restrictions through a mounted Redis ACL file
- NetworkPolicy ingress only from pods labeled `dd.dev/redis-cache-client: 'true'`

The in-cluster address is `dd-redis-cache.default.svc.cluster.local:6379`.

## Future persistence

If Redis starts holding state that must survive pod restarts or node replacement, convert the
workload from a cache-only Deployment into a persistent Redis service before storing durable data.
At minimum, that future change should:

- use a `StatefulSet` with a `PersistentVolumeClaim`
- enable AOF persistence with `appendonly yes`
- decide whether periodic RDB snapshots are also needed
- document backup, restore, and retention expectations
- add a rollout plan for migrating existing clients from disposable cache semantics

Until then, services should treat Redis values as rebuildable and keep the source of truth in
Postgres or another durable service-specific store.
