# Durable worker Rust SDK delivery record

Status: in review

Repository: [`ORESoftware/k8s-cluster`](https://github.com/ORESoftware/k8s-cluster)

Integration branch: `dev`

Milestone: M3 SDK fleet

Component: Worker SDK

Linear: [DEN-2392](https://linear.app/denman/issue/DEN-2392/add-rust-sdk-and-conformance-for-durable-worker-runtime)

GitHub issue: [#1021](https://github.com/ORESoftware/k8s-cluster/issues/1021)

Parent work: [DEN-2253](https://linear.app/denman/issue/DEN-2253/add-go-rust-dart-gleam-and-erlang-durable-worker-sdk-conformance) / [#980](https://github.com/ORESoftware/k8s-cluster/issues/980)

## Outcome

`remote/worker-sdks/rust/durable-worker` adds a hand-authored lifecycle SDK rather than another generated endpoint wrapper. It owns the protocol behavior that generated OpenAPI clients cannot safely infer:

- retries only where the request has a protocol identity;
- no automatic retry of ambiguous worker polls, signals, or unbound submissions;
- redirect refusal and bounded response bodies;
- worker registration, drain state, and TTL heartbeats;
- independent renewable step heartbeats;
- bounded local slot admission;
- deterministic progress chunk IDs scoped to step and lease generation;
- cancellation after heartbeat failure or fencing;
- suppression of completion and failure under a stale lease generation.

The default transport uses reqwest with rustls, no environment proxy inheritance, and disabled redirects. The public transport trait permits deterministic tests and specialized internal adapters without changing worker lifecycle semantics.

## Verification and publication

The focused workflow runs:

- Rust 1.85.0 and current stable;
- `cargo fmt`;
- clippy with warnings denied;
- all client, worker, and shared-fixture tests;
- repeated lease-fencing cancellation stress;
- dependency-duplication inventory;
- repository contract and credential-shape scans.

After merge to `dev`, the push workflow publishes:

```text
durable-worker-rust-sdk-<merge-sha>.tgz
durable-worker-rust-sdk-<merge-sha>.tgz.sha256
```

The archive is normalized by path order, timestamp, owner, group, and gzip metadata before hashing.

## GitHub Project record

Use the organization Project fields as follows:

| Field | Value |
| --- | --- |
| Status | In review until merged; Done after exact-head merge evidence |
| Milestone | M3 SDK fleet |
| Component | Worker SDK |
| Risk | Medium |
| Target | Current |
| Linear | DEN-2392 |
| Repository | ORESoftware/k8s-cluster |
| Issue | #1021 |

The PR URL, merge SHA, artifact name, artifact digest, and final workflow conclusion must be recorded after merge. GitHub and Linear issues remain open until GitHub reports `merged: true`.

## Remaining M3 work

The Rust slice does not close M3. Remaining work includes Dart, Gleam, Erlang/Elixir interoperability, broker/build adapters, and additional cross-language restart and cancellation fixtures.
