# GHA continuity repository publication v2

This broker publishes two public repositories in the `gha-indie-worker`
organization from immutable `ORESoftware/k8s-cluster` commit
`5cfac43c6900898f36f588d044ca34083da1c726`:

- `gha-clone-server.rs` from `remote/deployments/gha-clone-server-rs`;
- `gha-indie-worker.rs` from `remote/deployments/build-server-rs`.

The planner/compiler/router and the fixed-profile execution worker remain separate
repositories. GitHub-hosted Actions and ARC remain the native-semantics lane.

## Empty-repository bootstrap

GitHub returns HTTP 409 for Git Database blob creation until a repository has its
first commit. The v2 publisher therefore creates one exact
`.gha-indie-bootstrap` file through the Contents API before cloning or writing Git
objects. It permits only two pre-existing states: the exact bootstrap-only tree or
the exact completed import tree. Any other history fails closed. The reviewed
import is pushed as a normal fast-forward and removes the bootstrap file.

## Credential boundary

The trusted-main `pull_request_target` workflow accepts only an owner-created,
same-repository, draft, one-commit carrier with one exact seven-line marker. It
checks out only the pinned trusted source commit. A fresh RSA-3072 key is generated
inside the ephemeral runner for each invocation; the already-disclosed token is
accepted only as RSA-OAEP-SHA256/MGF1-SHA256 ciphertext. The plaintext is masked,
kept out of Git, comments, outputs, artifacts, and environment files, and erased
on exit after validating the `ORESoftware` identity and active organization-admin
membership.

## Import and verification

The publisher rejects source modes other than regular or executable files,
preserves bytes and executable bits, adds `SOURCE_PROVENANCE.md`, pushes without
force, verifies the remote `main` commit and provenance, confirms the bootstrap
file is absent, configures topics, and attempts review-protected `main` branches.
The carrier closes only after both repositories pass live verification.
