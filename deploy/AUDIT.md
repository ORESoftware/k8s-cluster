# live-mutex-rs (BrokerRaft) — cluster audit findings

Findings from building the `Dockerfile.raft` image and running a real 3-node
BrokerRaft cluster on k3d (one pod per k8s node), driving it through the
round-robin client Service, and killing the leader.

## What the live cluster proves

- The image builds and 3 pods form a Raft cluster across 3 separate k8s nodes,
  each with its own durable `volumeClaimTemplate` data dir.
- A leader is elected (`/raft/leaderz` / `/raft/status` → `leaderId`).
- **Liveness + follower proxy:** 8 concurrent clients drove acquire/release of a
  single hot lock through the round-robin client Service (so requests landing on
  followers were proxied to the leader). All succeeded.
- **Safety:** every acquire returned a fencing token, and no token was ever
  handed out twice → no double-grant → mutual exclusion held. (See
  `deploy/k8s/verify-job.yaml`.)
- **HA failover (the headline feature):** killing the leader pod elected a NEW
  leader within seconds, lock liveness was restored, and fencing tokens stayed
  strictly monotonic across the failover — no token rollback or reuse. (See
  `deploy/failover-test.sh`.)

## Contrast with the maekawa repos (good news)

Unlike `live-mutex-mills.rs` / `live-mutex.distributed`, the BrokerRaft HTTP
`lockUuid` is **globally valid**: acquire and release can hit different nodes
(the round-robin client Service works, with followers proxying to the leader),
because the lock state lives in the replicated log rather than in per-node
coordinator state. So no acquire/release session affinity is required here.

## Finding — locks acquired via HTTP do not auto-release on `ttlMs` expiry

A lock acquired with `POST /v1/lock {"key":..,"ttlMs":1500}` and **not** explicitly
unlocked stayed held well beyond the TTL (observed held for >10 minutes, and a
fresh-key controlled test stayed held for the full 9s polling window — past both
the 1500ms request TTL and the 4000ms `default_ttl_ms`). The TTL sweeper is
enabled by default (`ttl_sweep_interval` defaults to 10ms even when the config
omits it — `src/config.rs:266`), and `docs/raft.md` shows TTL **deadlines** are
tracked and snapshotted — yet wall-clock auto-release of a held lock did not
occur in this deployment.

**Why it matters.** A client that crashes (or simply forgets) without calling
`/v1/unlock` holds the lock indefinitely — the same class of liveness hazard as
the maekawa repos' client-crash finding. `ttlMs` looks like the intended guard
but did not function as one here.

**Needs confirmation:** whether held-lock TTL expiry in BrokerRaft is (a) intended
but gated/unwired in this build, (b) requires an explicit holder heartbeat /
renew call, or (c) a real gap. Wall-clock expiry inside a replicated state
machine is genuinely subtle (the leader must propose deterministic expiry
entries), so this is a plausible incomplete corner rather than a core-safety bug
— the fencing-token backstop still prevents a stale holder from corrupting a
resource. Repro: `deploy/` up, then
`POST /v1/lock {"key":"k","ttlMs":1500}` and poll re-acquire.

## Minor

- The verify Job shares the `app: live-mutex-rs-raft` label; `failover-test.sh`
  filters pod names to `live-mutex-rs-raft-[0-9]+` so it never tries to drive the
  Job pod.
