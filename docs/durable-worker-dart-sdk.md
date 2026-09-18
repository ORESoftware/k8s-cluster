# Durable Worker Runtime — Dart SDK Delivery

Status: delivered  
Milestone: M3 SDK fleet  
Linear: [DEN-2464](https://linear.app/denman/issue/DEN-2464/add-dart-sdk-and-conformance-for-durable-worker-runtime)  
GitHub issue: [#1163](https://github.com/ORESoftware/k8s-cluster/issues/1163)  
Delivery PR: [#1586](https://github.com/ORESoftware/k8s-cluster/pull/1586) (successor to stale #1169)

## Purpose

`remote/worker-sdks/dart/durable_worker` is the hand-authored Dart lifecycle
SDK for `dd-durable-worker-server`. It remains separate from generated OpenAPI
clients because worker safety depends on stateful behavior that an endpoint
generator cannot infer: retry identity, renewable leases, cancellation,
progress identity, bounded concurrency, and stale-terminal suppression.

The runtime contract is at-least-once. Dart handlers must make external effects
idempotent or fence them with `TaskContext.fencingToken`.

## Initial delivery slice

- dependency-free Dart 3.4+ HTTP client using `dart:io`;
- task and run submission, run controls, signals, and lookup;
- worker registration, long polling, worker heartbeat, and drain;
- start, heartbeat, progress, completion, and failure mutations;
- automatic retries only for operations with stable protocol identities;
- no retry for worker polls, signals, or unbound submissions;
- redirect refusal before credentials can cross origins;
- bounded response bodies and structured protocol/transport errors;
- bounded worker slots with independent worker and step heartbeats;
- progress IDs scoped to `{stepId}:{leaseGeneration}:{sequence}`;
- lease-loss and heartbeat-uncertainty cancellation;
- stale completion/failure suppression;
- shared protocol fixture, local HTTP contract tests, and repeated fencing stress.

## Validation

The focused workflow runs on Dart 3.4.0 and 3.12.2 with:

- committed dependency-free lock enforcement;
- canonical formatting followed by a clean-tree assertion;
- analyzer infos and warnings treated as failures;
- client, worker, fixture, redirect, retry, response-boundary, and fencing tests;
- fifty repeated lease-fencing cancellation passes on the current toolchain;
- read-only, non-persistent checkout and credential-shape scanning;
- deterministic source archive and SHA-256 publication after a trusted merge.

A one-run formatter publisher was used only because direct Git transport was
unavailable in the execution environment. It was constrained to the three
reviewed formatter paths, committed canonical Dart 3.12.2 output, and was
removed immediately afterward.

The first real analyzer pass then identified one nullable throw and two closure
wrappers. A second one-run repair was constrained to `lib/src/worker.dart`; it
had to pass canonical formatting, the fatal analyzer, and the complete client,
fixture, worker, and fencing harness before publishing. That workflow was also
removed immediately. The permanent workflow is `contents: read` with non-persistent checkout. The
successor delivery head `9df684208898e862fe4ac746b2b04495a4ab46e8`
passed the clean-head Dart 3.4.0 and 3.12.2 matrix, fatal analyzer, runtime and
repository-contract tests, fifty repeated fencing passes, credential scan, and
clean-tree checks. PR #1586 merged that reviewed content as
`81bf0548fb68ada82fd3d01d4456219cd4e71387`.

The reviewed lock artifact is
`durable-worker-dart-lock-b66a6a6bc69d4794d287f6fb5af94a509f7e12bf`
with GitHub artifact digest
`sha256:a49f6e81d8f80783f507d1dba47f1cf96bb290ccb0d9e29651ee0a86442f72a6`.
It is lock-review evidence, not a package-registry or source release. The
deterministic source archive remains pending the first trusted `dev` push that
can execute the push-only publication job introduced by #1586.

## Organization project record

| Field | Value |
| --- | --- |
| Organization | ORESoftware |
| GitHub Project | `ORESoftware-project` (`orgs/ORESoftware/projects/1`) |
| Repository | `k8s-cluster` |
| Component | Worker SDK |
| Milestone | M3 SDK fleet |
| Linear issue | DEN-2464 |
| GitHub issue | #1163 |
| Pull request | #1586 (successor to #1169) |
| Risk | Medium |
| Status | Done |

Reviewed exact head and merge evidence are recorded above. The trusted source
archive identity and digest must still be added after the first qualifying
`dev` push publication; do not represent the reviewed lock artifact as that
source release.
