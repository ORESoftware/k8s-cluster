# Durable worker SDK fleet contract

Status: active

The lifecycle-aware durable-worker SDKs are hand-authored stateful clients. **Generated OpenAPI clients are not lifecycle SDKs** and live under `remote/api-sdks`; generated endpoint bindings must not be substituted for worker retry, lease, cancellation, drain, progress, or fencing behavior.

The versioned fleet authority is `remote/worker-sdks/fleet-v1.json`. It names the landed lifecycle SDKs, their package roots and minimum runtimes, and the mandatory conformance dimensions inherited from `remote/worker-sdks/fixtures/durable-worker-protocol-v1.json`.

## Fleet gate

The aggregate workflow is intentionally additive to each language's focused workflow. It uses read-only, non-persistent checkout and executes one native check for every fleet member. Language steps use `continue-on-error` only so the workflow can collect every result and emit evidence; the final gate still fails when any setup, repository contract, credential scan, or native language check fails.

The report contains only deterministic source identity, manifest/fixture digests, and check outcomes. It excludes timestamps and payload data. Every run writes:

- `sdk-fleet-report.json`;
- `sdk-fleet-report.sha256`.

The pair is uploaded as a GitHub **Actions artifact** even when a language check fails, provided checkout/report generation is available. An Actions artifact is **not a package-registry release**.

## Add a language

1. Land the independently reviewable lifecycle SDK with its own native workflow and protocol/fencing tests.
2. Extend `durable-worker-protocol-v1.json` so `handAuthoredSdks` includes the language without changing the existing safety dimensions.
3. Add one entry to `fleet-v1.json` with a unique language, lifecycle root, package marker, minimum runtime, native check, and the complete shared capability set.
4. Add the language's pinned toolchain setup and native check to `durable-worker-sdk-fleet.yml`.
5. Extend `durable-worker-sdk-fleet.test.mjs` and the language-specific shared fixture assertions.
6. Keep generated API clients under `remote/api-sdks`; never move generated code into a lifecycle SDK root to satisfy this gate.
7. Require exact-head focused CI before merging. A report artifact records evidence; it never waives a failing native contract.

Gleam (#1164 / DEN-2480) is the next planned fleet addition. Erlang/Elixir interoperability (#1165 / DEN-2482) remains a separately reviewable downstream lane.
