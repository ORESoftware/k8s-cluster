# Formal-methods state-machine inventory

Fiducia keeps executable specifications beside the production code they
describe. Each adopter exposes a default `formal/fm.toml` schema-v1 manifest
consumed by the incubating `fmctl` runner; repositories with another model use
an explicit secondary manifest (currently brain's
`formal/fm-reconfiguration.toml`). Until `fmctl` is extracted into the shared
DEN-580 repository, each product's pinned workflow executes the same
manifest-declared toolchain and exploration sizes directly.

The compatibility baseline is
`opto-sync/opto-sync-clients@c2146ef9f054d24e1488c216547852aa148285cf`
under `tools/fmctl`. All five manifests in this inventory pass that binary's
strict `validate` command; DEN-580 owns preserving the contract during
extraction.

The monorepo validates manifests at reviewed gitlink commits with
`scripts/check-formal-methods-manifests.py --self-test`. Validation mirrors
`fmctl`'s strict schema-v1 fields, checks every referenced spec and adapter
target, rejects path traversal and unknown fields, validates execution limits,
and requires active adapters to declare executable command arrays. Planned
adapters remain explicit without being reported as implementation conformance.

## Current verified surfaces

| Linear | Repository and model | State machine and principal invariant | Evidence level |
|---|---|---|---|
| `DEN-566` | `fiducia-node.rs` / `union-lock-v2` | Atomic multi-key acquisition, FIFO reservations, retry/cancel/expiry, monotonic fencing; `union_lock_safety` | Bounded Apalache model checking, independent Rust refinement, and generated ITF replay against production |
| `DEN-635` | `fiducia-ai-agent-bridge.rs` / `compatibility-file-lease-v1` | Recursive repository-path overlap, durable token floor, restart, journal failure, exhaustion; `file_lease_safety` | Exhaustive finite TLC model checking and bounded public-API Rust refinement; ITF is generated but not replayed |
| `DEN-569` | `fiducia-brain.rs` / `brain-membership` | Heartbeat sequence monotonicity, sticky drain, timeout/oracle reconciliation, one-shot death; `membership_safety` | Exhaustive finite TLC model checking and bounded independent Rust refinement; ITF replay remains planned |
| `DEN-569` | `fiducia-brain.rs` / `brain-reconfiguration` | Incomplete/degraded membership holds, learner-before-voter, quorum floor, leader-safe removal; `reconfiguration_safety` | Exhaustive finite TLC model checking; production scheduler/placement refinement and ITF replay remain planned |
| `DEN-651` | `fiducia-ai-agent-control-plane` / `supervisor-policy-v1` | Recovery precedence, stale-generation fencing, execution/agent lifecycle authority, completion gates; `supervisor_safety` | Exhaustive finite TLC model checking and exhaustive independent Rust policy refinement; ITF is generated but not replayed |

“Exhaustive” above always refers to the finite domains recorded in each
manifest. It is not a claim about an unbounded production deployment.

## Claim boundaries

The current models prove finite safety properties and bounded implementation
refinement only where the table says so. They do not collectively prove:

- Raft transport, elections, joint consensus, log replication, or snapshots;
- PostgreSQL isolation, journal filesystem semantics, or disk-fault behavior;
- unbounded clock/sequence/token domains;
- eventual progress without explicit fairness and environment assumptions;
- network partitions, process termination, credential-provider behavior, or
  Kubernetes control-plane correctness.

Those boundaries are intentional. A new “implemented” adapter may be declared
only when its target exists and the manifest validator can prove the target is
present at the reviewed gitlink. Generated ITF without a production replay
adapter must remain described as trace evidence, not implementation
conformance.

## Next state machines

The next highest-consequence slices under `DEN-80` are:

1. `fiducia-brain` scheduler/placement ITF replay, leader failover, restart
   adoption, degraded replication, and explicit finite fairness scenarios;
2. brain and node Raft stale-leader, committed-prefix, snapshot-install, and
   membership-change safety;
3. effects escrow/idempotency across duplicate delivery and failover;
4. task claim/reclaim and cron-fire ownership.
