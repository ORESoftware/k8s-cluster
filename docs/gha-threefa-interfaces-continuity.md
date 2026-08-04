# 3FA interfaces bounded CI continuity

Linear: DEN-539  
Architecture parents: DEN-1549, DEN-1550

This onboarding adds `3FA-app/3fa-interfaces` to the independent continuity lane without claiming full GitHub Actions parity or weakening the repository's native release gates.

## Exact admission

The reviewed boundary is:

- repository: `3FA-app/3fa-interfaces`;
- clone URL: `=https://github.com/3FA-app/3fa-interfaces.git`;
- workflow: `.github/workflows/gha-clone-contracts.yml`;
- revision: immutable lowercase 40-hex commit only;
- execution: fixed `dd-build-server` profiles only.

The leading `=` is significant. Profile execution must match the complete canonical HTTPS repository URL; a neighboring repository in the `3FA-app` organization is not admitted by this change.

## Mirrored DAG

The bounded workflow compiles to two submissions:

1. `node_contracts` → `node-hardened-test`
2. `generated_rust` → `rust-generated-verify`, after `node_contracts`

The generated-Rust profile accepts only this exact command sequence:

```sh
cargo generate-lockfile --manifest-path generated/rust/Cargo.toml
cargo fmt --manifest-path generated/rust/Cargo.toml -- --check
cargo clippy --locked --manifest-path generated/rust/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path generated/rust/Cargo.toml --all-targets
```

Reordering a command, adding another Cargo operation, changing the manifest, using a mutable action reference, changing the workflow path, or using another repository prevents independent execution before a build-server request is emitted.

## Deliberately native-only work

These remain GitHub-hosted or official ARC responsibilities:

- TLA+ lifecycle model checking;
- Dart analysis and examples;
- package dry-runs, publication, signing, and release evidence;
- service containers, secrets, environments, approvals, OIDC, deployment, and mobile/native hardware jobs.

The current TLA+ script downloads a pinned and checksummed jar. The independent lane does not reproduce that network bootstrap or reinterpret a missing formal proof as success.

## Private source credentials

Live execution requires a short-lived GitHub App installation token restricted to `3FA-app/3fa-interfaces` with `contents:read`. The token must be mounted or supplied through the reviewed ExternalSecret/App boundary and removed after clone. It must not be written into Git config, URLs, build requests, status payloads, logs, artifacts, or Linear.

A classic PAT is not an accepted fallback.

## Activation sequence

1. Merge the repository workflow and its repository-owned safety test.
2. Merge this exact profile/admission change.
3. Publish and pin the continuity images by immutable digest with SBOM, provenance, and vulnerability evidence.
4. Install the reviewed source-read GitHub App on only the admitted repository.
5. Keep GitOps execution disabled and replicas at zero during static validation.
6. Run plan-only validation at the merged repository commit.
7. Scale one router and one executor for a controlled run.
8. Require exactly two fixed-profile submissions and one successful status chain.
9. Remove source credentials, scale back to zero, and preserve redacted evidence.
10. Separately rerun the native TLA+, package, Dart, and release workflows on GitHub-hosted or ARC runners.

## Evidence boundary

A successful independent run proves only the bounded Node and generated-Rust contracts at the exact commit. It does not prove the formal models, publication, service-container behavior, native mobile builds, production credentials, or release readiness.
