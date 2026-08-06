# Fiducia Raft durability, migration, backup, and restore runbook

Status: PVC contract implemented; production migration and clean-room restore evidence still required.

Tracks: DEN-437

## Storage contract

`fiducia-node` owns authoritative Raft KV state at `/var/lib/fiducia`. The rendered StatefulSet must provide that mount through a stable `data` `volumeClaimTemplate`; an `emptyDir` is forbidden for this volume.

The baseline requests:

- StorageClass: `gp3`;
- capacity: `20Gi` per voting member;
- access mode: `ReadWriteOnce`;
- volume mode: `Filesystem`;
- deletion and scale-down retention: `Retain`;
- expansion requirement: the installed `gp3` class must have `allowVolumeExpansion: true` before rollout;
- encryption requirement: AWS EBS encryption at rest or the provider-equivalent encrypted backing store;
- topology requirement: one voter per failure domain where the cluster has enough nodes;
- initial alert thresholds: warning at 70% usage, critical at 85%, sustained p99 write latency above 25 ms, member lag above the agreed Raft catch-up bound, and any unbound/lost PVC.

The Hetzner bootstrap currently aliases its local-path provisioner as `gp3`. That preserves manifest compatibility but does **not** survive node replacement. A laptop or Hetzner production rollout therefore remains blocked until the backing class is replaced with durable network/block storage or the node-loss limitation is explicitly accepted for a temporary environment.

## Pre-migration gates

Before changing any voter:

1. Record the current StatefulSet revision, member IDs, leader, commit/applied indexes, healthy-voter count, PVC inventory, active KV encryption key ID, and all historical key IDs required by live data or snapshots.
2. Verify three healthy voting members and a functioning quorum.
3. Verify the target StorageClass exists, expands, encrypts at rest, and can bind in every intended failure domain.
4. Create and verify an encrypted backup in an independent destination.
5. Confirm the rollback operator, restore approver, and key-custody approver are available.
6. Freeze unrelated Fiducia rollouts.

Stop immediately on loss of quorum, an unavailable backup, a missing encryption key ID, member divergence, unexpected leader churn, failed PVC binding, or failed application readiness.

## One-voter-at-a-time migration

Migrate exactly **one voting member at a time**, starting with the highest ordinal and never replacing the current leader intentionally.

For each ordinal:

1. Confirm the other two voters are healthy and caught up.
2. Transfer leadership away from the target when supported; otherwise wait until it is a follower.
3. Scale or delete only the target pod, leaving its PVC retained.
4. Bind the new `data-<statefulset>-<ordinal>` PVC or copy the existing durable state using the reviewed application-supported procedure. Never copy a live, mutating data directory with an ordinary filesystem copy.
5. Start the replacement member and verify identity, membership, log catch-up, commit/applied index convergence, readiness, and representative reads/CAS writes.
6. Observe a stability window before continuing to the next ordinal.
7. Record evidence and the exact rollback point.

Do not restart, replace, or migrate multiple voters simultaneously.

## Rollback

On a failed replacement:

- stop the replacement before it can form an unintended cluster;
- retain both old and new PVCs;
- restore the prior StatefulSet revision or reattach the last known-good volume only under the documented member-identity procedure;
- verify the surviving quorum and indexes before resuming traffic;
- do not delete any PVC, snapshot, WAL segment, or encryption key;
- open an incident record containing timestamps and identifier-safe diagnostics.

A rollback that requires replacing a second voter is an incident, not a normal migration step.

## Backup contract

The node implementation must expose or document an application-consistent snapshot/WAL procedure before automated backups are enabled. Filesystem-level copies of a live Raft store are not accepted unless the storage engine explicitly guarantees their consistency.

Required backup properties:

- encrypted before leaving the node or storage backend;
- independently stored from the live cluster and its PV lifecycle;
- immutable or object-locked for the configured retention window;
- versioned with cluster ID, member set, applied index/revision, schema version, checksum, creation time, active key ID, and the set of historical encryption key IDs required for decryption;
- least-privilege writer credentials and separately authorized restore credentials;
- monitored for age, failure, size anomaly, checksum failure, and missing key IDs;
- deletion requiring documented dual approval.

Initial policy target: hourly incremental/WAL capture where supported, daily full snapshots, 35 daily restore points, 12 monthly restore points, and a quarterly long-term point. Final RPO/RTO must be approved from measured restore results.

## Clean-room restore production gate

Production reliance on Fiducia as a secret authority is blocked until an independent **clean-room restore** succeeds:

1. Provision a new isolated cluster and new PVCs without access to the live Raft data directory.
2. Provide only the selected encrypted backup, its manifest/checksums, and the approved encryption keyring.
3. Restore through the application-supported procedure.
4. Start a valid quorum and verify cluster/member identity rules.
5. Compare representative values, revisions, CAS success/failure behavior, auth key records, token revocation state, rotation state, and encrypted-value key IDs.
6. Revoke restore credentials after the drill.
7. Attach command output with secret values redacted, checksums, timings, failure observations, and approver sign-off to DEN-437.

## Required disaster drills

- replace one member on a different node and prove catch-up;
- lose one member while preserving quorum and KV availability;
- interrupt a migration and roll back without touching a second voter;
- restore into a clean cluster and prove revision/CAS and auth/rotation semantics;
- prove an intentionally missing historical key ID causes a fail-closed restore, then restore successfully with the complete keyring.

The GitOps and static-test changes in this repository enforce the durable PVC shape. They do not substitute for live migration, backup, or restore evidence.
