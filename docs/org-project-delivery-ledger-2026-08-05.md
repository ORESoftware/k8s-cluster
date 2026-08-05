# Organization project delivery ledger — August 5, 2026

This ledger records the current reviewed delivery state for the three GitHub organizations that own the Fiducia queue/Raft work and the bounded GitHub Actions continuity lane. It complements the canonical 41-row registry in [`ops/registries/portfolio-project-links.csv`](../ops/registries/portfolio-project-links.csv).

The registry owns immutable routing identity. This ledger owns delivery evidence and cross-organization responsibility boundaries.

## Project routing

| Portfolio key | GitHub organization | GitHub Project | Linear project | Delivery ownership |
|---|---|---|---|---|
| `fiducia-cloud` | [`fiducia-cloud`](https://github.com/fiducia-cloud) | [`fiducia-cloud-project` #1](https://github.com/orgs/fiducia-cloud/projects/1) | [`github.com/fiducia-cloud`](https://linear.app/denman/project/githubcomfiducia-cloud-8fd5e1bec9d3) | Queue, lock/lease state machine, Raft authority, fencing, durable assignment, and distributed fault evidence. |
| `oresoftware` | [`ORESoftware`](https://github.com/ORESoftware) | [`ORESoftware` Project #1](https://github.com/orgs/ORESoftware/projects/1) | [`github.com/ORESoftware/k8s-cluster`](https://linear.app/denman/project/githubcomoresoftwarek8s-cluster-c9e32add54f1) | Shared Kubernetes, official ARC, bounded clone-server integration, executor routing, fixed-profile build substrate, and cross-provider activation. |
| `gha-indie-worker` | [`gha-indie-worker`](https://github.com/gha-indie-worker) | [`gha-indie-worker-project` #1](https://github.com/orgs/gha-indie-worker/projects/1) | [`github.com/gha-indie-worker`](https://linear.app/denman/project/githubcomgha-indie-worker-941d4102f7dc) | Standalone bounded clone-server source, repository-local CI, platform-boundary contracts, and extraction provenance. |

## `fiducia-cloud`

### Final merges

| Repository | Pull request | Exact validated head | Merge commit |
|---|---:|---|---|
| `fiducia-cloud/fiducia-node.rs` | [#29](https://github.com/fiducia-cloud/fiducia-node.rs/pull/29) | `3f07402474c2edc98f17a87e951e6116fad1d80d` | `b9177646f9c69c67b76b3fbee9fded9b585e9c0c` |
| `fiducia-cloud/fiducia-brain.rs` | [#25](https://github.com/fiducia-cloud/fiducia-brain.rs/pull/25) | `8acbfe76bb03f9a693acdbe0f4649bc8851f2ab1` | `588d1bc2d6a61514ef0d036280f9cde20fb6284d` |

The queue container remains a deterministic data structure; logical queue authority belongs to committed Raft commands and validated state-machine snapshots. There is no second queue WAL. Process-local list links, slab indexes, hash buckets, waiters, and response channels are not durability authority.

`fiducia-node.rs#29` passed 265 product tests, 129 formal/refinement tests, a deterministic 25,000-operation differential queue test, strict Clippy, permanent CI, and Nix validation. `fiducia-brain.rs#25` passed formatting, all-target tests, strict Clippy, permanent CI, and formal-method workflows.

### Remaining project items

- per-member mTLS/SPIFFE identity;
- real multi-process partition, stale-leader, transfer, delayed/duplicate-delivery, and restart tests;
- restart-durable lost-response retries using request IDs and fencing tokens;
- sustained contention/fairness/starvation and memory-soak evidence;
- PVC backup, restore, snapshot, and log-replay evidence.

## `ORESoftware`

### Final merges

| Capability | Pull request | Merge commit |
|---|---:|---|
| Atomic direct and webhook-batch run admission | [#751](https://github.com/ORESoftware/k8s-cluster/pull/751) | `a14072064f25d7b49807656d4231f93d335a6d55` |
| Semantic transport, origin, runtime-bound, and build-identity union | [#764](https://github.com/ORESoftware/k8s-cluster/pull/764) | `b827d1fde69bdfc5acfeb9d8a785f184c3ce5505` |
| Real-process redirect, poll-before-trust, identity, and zero-bound tests | [#843](https://github.com/ORESoftware/k8s-cluster/pull/843) | `fee1b96e90cd340fb65da26fd4c785a8bb1eeb1c` |
| Raw exact-profile policy byte hardening | [#844](https://github.com/ORESoftware/k8s-cluster/pull/844) | `a9776dce110a348c531dcab22244847c3e419184` |
| Daily 41-organization project-link reconciliation | [#877](https://github.com/ORESoftware/k8s-cluster/pull/877) | `74bd901418c61bfe48a5e0480b2d577564100179` |

Official Actions Runner Controller remains the native-semantics lane. The independent clone-server lane accepts only reviewed repositories, immutable revisions, direct workflow paths, a bounded YAML subset, and fixed build profiles. It does not claim full GitHub Actions parity.

Provider selection occurs before submission. After the first submission attempt, uncertain acceptance or polling failure remains pinned and must not fan out to another provider. Keep one active clone-server/router replica until assignment, delivery claims, build/artifact identity, and ownership are durable and Fiducia-fenced.

### Remaining project items

- authoritative organization Actions billing read;
- least-privilege Apps for billing, ARC registration, private-source read, and narrowly scoped capacity mutation;
- immutable signed runner/executor images with SBOM, provenance, and vulnerability evidence;
- exact-SHA AWS and Hetzner official ARC smokes;
- provider-loss, cancellation, cleanup, and rollback drills;
- durable multi-replica assignment and delivery ownership.

## `gha-indie-worker`

The standalone source authority is:

```text
ORESoftware/k8s-cluster@e75a654bfd527500a3a2ef4ceb1836e78e14a7a6
remote/deployments/gha-clone-server-rs
```

[`gha-indie-worker/gha-clone-server.rs#2`](https://github.com/gha-indie-worker/gha-clone-server.rs/pull/2) is exact-head green at `5d8f4c00ea359c495b4bc997b29ce22cfc9c9c4f`, non-draft, mergeable, and has no unresolved review threads. Squash auto-merge is enabled. Branch protection still requires one independent write-access approval; that requirement is not bypassed.

The six-file product adds repository-local pinned CI, a repository-owned meta fixture, machine-readable and human-readable platform-boundary contracts, architecture-contract tests, and corrected standalone self-test ownership.

### Remaining project items

- independent approval and automatic squash merge of PR #2;
- protect `main` with repository-local CI and secret scanning;
- retain immutable source provenance and complete-tree comparison for future extraction updates;
- keep the supported YAML subset fail-closed;
- do not scale webhook/run state beyond one active replica until Fiducia-backed durable claims and fencing are certified.

## Cross-project rule

Do not duplicate ownership across boards:

- queue/Raft/fencing and durable distributed ownership belong to `fiducia-cloud`;
- cluster, ARC, router, fixed-profile executor, and cross-provider operations belong to `ORESoftware`;
- standalone clone-server source, repository CI, and extraction provenance belong to `gha-indie-worker`.

The daily project reconciler preserves human-authored GitHub Project and Linear descriptions outside bounded managed blocks. The separate Linear-next-steps workflow may create or update selected GitHub Project items using the same canonical `portfolio_key`.