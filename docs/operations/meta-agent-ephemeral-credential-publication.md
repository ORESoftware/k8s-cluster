# Meta Agent repository publication with an ephemeral credential challenge

This is an emergency bootstrap path for the first real repository in the
`meta-agents-demo` organization. It exists because an empty organization cannot
receive source through an ordinary repository PR, while the connected GitHub
App currently exposes repository reads but not repository creation.

The owner credential is **never committed**, added to a GitHub Actions secret,
placed in a workflow input, written to an artifact, or copied into Linear. A
trusted `pull_request_target` workflow on `ORESoftware/k8s-cluster` generates an
ephemeral RSA key pair. Only the public key and a random nonce are posted to the
execution carrier. The operator replies with RSA-OAEP-SHA256 ciphertext. The
private key and decrypted value exist only in the runner's temporary directory
and process memory and are destroyed on exit.

## Trust boundary

The trusted workflow accepts only a draft, same-repository, ORESoftware-owned
one-commit carrier with one four-line non-executable marker. The marker pins the
target repository, the carrier's trusted `main` parent, the reviewed bundle
digest, and the protocol version. Current `main` must be identical to or ahead
of that parent; PR-controlled code is never checked out or executed.

After decryption, the workflow verifies the credential identifies exactly
`ORESoftware` and has active admin membership in `meta-agents-demo` before any
repository mutation. It reconstructs the exact recovered Git history from the
pinned source commit, verifies the bundle and publisher digests, creates the
public repository idempotently, pushes only the reviewed `main` and feature
refs without force, verifies the live metadata and SHAs, and opens the normal
implementation PR.

## Rotation and audit

Because a credential was disclosed in chat, rotate the credential immediately
after the repository and exact refs are verified. The execution carrier,
challenge, encrypted response, workflow run, target repository metadata, exact
ref SHAs, implementation PR, and Linear issue comments form the audit trail.
The ciphertext is not reusable because each execution has a fresh private key
and nonce.

## Linear ownership

DEN-1057 owns the canonical `meta-agent-control-plane.rs` repository and its
implementation review. DEN-319 owns the broader missing-repository publication
system. Neither issue is complete until live GitHub reads prove the repository,
default branch, visibility, exact refs, tests, and review PR.
