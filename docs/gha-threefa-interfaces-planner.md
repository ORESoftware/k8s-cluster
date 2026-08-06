# 3FA interfaces bounded continuity planner

This continuity path classifies one credential-free workflow from one exact private repository. It does not enable the zero-replica deployment, fetch private source through a live GitHub App, or alter GitHub required checks.

## Exact source identity

```text
repository: 3FA-app/3fa-interfaces
workflow:   .github/workflows/gha-clone-contracts.yml
revision:   exact 40-hex commit SHA only for independent execution
```

The GitOps allowlist does not grant the `3FA-app` organization, sibling repositories, other workflow paths, branch names, tags, or nested workflow files.

## Compiled graph

The reviewed fixture has two jobs:

```text
node_contracts -> generated_rust
```

`node_contracts` maps only to `node-hardened-test` and must contain the exact lifecycle-script-free locked install followed by the repository test script:

```text
npm ci --ignore-scripts
npm test
```

`generated_rust` maps only to `rust-generated-verify` and must contain this exact sequence with no extra or reordered commands:

```text
cargo generate-lockfile --manifest-path generated/rust/Cargo.toml
cargo fmt --manifest-path generated/rust/Cargo.toml -- --check
cargo clippy --locked --manifest-path generated/rust/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path generated/rust/Cargo.toml --all-targets
```

## Exact setup-action authority

For the registered 3FA tuple, setup actions are also an exact ordered allowlist:

```text
node_contracts:
  actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
  actions/setup-node@820762786026740c76f36085b0efc47a31fe5020

generated_rust:
  actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
  dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30
```

A mutable ref such as `@main` or `@stable`, a reordered sequence, a missing action, or any extra marketplace action makes the affected job independently unsupported. It cannot fall back to a generic Node or Rust profile. This enforcement is scoped to the exact 3FA repository/workflow tuple so unrelated existing fixtures retain their reviewed behavior.

Secret expressions, service or job containers, environment approvals, strategy matrices, custom shells, working directories, publication, and caller-selected commands also fail closed.

## Real-process execution evidence

The `threefa_http` integration starts the actual `gha-clone-server` binary with API execution enabled only inside the test process and points it at a recording local build-server mock.

It proves that one accepted immutable request produces exactly two authenticated build submissions in deterministic topological order:

```text
node_contracts  -> node-hardened-test
generated_rust -> rust-generated-verify
```

Every submission contains:

- `schemaVersion=build-server.v1`;
- `jobKind=run-profile`;
- exact `https://github.com/3FA-app/3fa-interfaces.git` repository URL;
- the exact immutable 40-hex revision;
- no caller command or image; and
- deterministic `gha-clone:{planId}:{jobId}` request identity.

Repeating the exact run reuses each job's request identity while keeping the Node and Rust job identities distinct. Mutable revisions, unreviewed sibling repositories, command-extended workflows, mutable action refs, and extra actions are rejected before the mock build server receives any submission.

This is execution-contract evidence against a mock build server; it is not a live private-source run. The GitOps deployment remains at zero replicas with API and webhook execution disabled.

## Independent authority

The build server independently enforces the exact repository-to-profile rule merged through DEN-539: `3FA-app/3fa-interfaces` may use only `node-hardened-test` and `rust-generated-verify`. The planner and dispatcher cannot widen that authority.

## Evidence required before live activation

The reviewed path must retain:

- Rust formatting and warnings-denied Clippy;
- all existing workflow parser, StreemPilot, transport, and auth regressions;
- 3FA planner and adversarial command/action tests;
- the real-process 3FA dispatch, retry, and zero-submission rejection tests;
- complete build-server profile/admission/idempotency/NATS tests;
- actionlint and complete continuity Kustomize render;
- exact repository/workflow static contracts; and
- credential-pattern rejection.

Live private-source execution still requires the least-privilege GitHub App, reconciled ExternalSecret, plan-only deployment evidence, and explicit activation review.
