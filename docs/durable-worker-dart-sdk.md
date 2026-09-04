# Durable Worker Runtime — Dart SDK Delivery

Status: in review  
Milestone: M3 SDK fleet  
Linear: [DEN-2464](https://linear.app/denman/issue/DEN-2464/add-dart-sdk-and-conformance-for-durable-worker-runtime)  
GitHub issue: [#1163](https://github.com/ORESoftware/k8s-cluster/issues/1163)  
Delivery PR: [#1169](https://github.com/ORESoftware/k8s-cluster/pull/1169)

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
removed immediately. The current branch contains only the permanent
`contents: read`, non-persistent workflow; readiness requires a clean-head
matrix after both removals.

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
| Pull request | #1169 |
| Risk | Medium |
| Status | In review |

This record must still be updated with the reviewed exact head, merge commit,
trusted push workflow, artifact name, and artifact digest before DEN-2464 and
#1163 are closed.
