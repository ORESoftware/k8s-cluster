# 3FA interfaces bounded continuity planner

This increment classifies one credential-free workflow from one exact private repository. It does not enable the continuity service, fetch private source, dispatch builds, or alter GitHub required checks.

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

The fixture pins checkout, Node setup, and Rust toolchain actions because the independent fixed profiles own the actual toolchain and command execution. This PR enforces exact run-command sequences and immutable repository revisions. General setup-action reference immutability is tracked separately so unrelated existing fixtures are not silently reclassified in this change.

Secret expressions, service or job containers, environment approvals, strategy matrices, custom shells, working directories, publication, and caller-selected commands fail closed.

## Separation from execution

This planner change only produces a deterministic plan and explicit rejection reasons. The deployment remains at zero replicas with API and webhook execution disabled. A later PR must prove real-process dispatch against a mock build server before any private-repository installation token or live source fetch is considered.

The build server independently enforces the exact repository-to-profile rule merged through DEN-539: `3FA-app/3fa-interfaces` may use only `node-hardened-test` and `rust-generated-verify`. The planner cannot widen that authority.

## Evidence required

The planner PR must pass:

- Rust formatting and warnings-denied Clippy;
- all existing workflow parser and transport/auth regressions;
- the 3FA fixture planner and adversarial tests;
- actionlint and complete continuity Kustomize render;
- exact repository/workflow static contract checks; and
- credential-pattern rejection.
