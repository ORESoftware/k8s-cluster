# Private Shared Auth introspection validation

`shared-auth/shared-auth-server.rs` pull request 30 carries the canonical
service-authenticated token-introspection and MFA `auth_time` contract. The
Shared Auth organization currently fails every exact-head job before runner
allocation: no command starts and no job log is produced. This temporary
workflow provides executable evidence inside a private repository whose hosted
Rust, PostgreSQL, security, regression, and Nix lanes are known to allocate and
pass. It is a one-release continuity boundary, not a permanent substitute for
restoring native Shared Auth runner allocation.

## Confidentiality boundary

Both this host repository and the Shared Auth source repository are private.
The trusted workflow verifies the host's GitHub API `private` flag before it
accepts a carrier. It never checks out or executes carrier content and never
publishes an artifact. Compiler, formatter, browser, audit, and container logs
therefore remain within the private repository boundary.

## Exact starting target

The one-use workflow accepts only:

- repository: `shared-auth/shared-auth-server.rs`;
- pull request: `30`;
- branch: `agent/shared-auth-auth-time-claim`;
- starting commit: `4148e5b96a448a20da00922cc62386455e211126`.

The carrier must be an owner-created, same-repository, draft, one-commit pull
request containing one exact six-line marker. Its parent must remain an ancestor
of current `main`.

## Deterministic documentation correction

Before testing, the trusted workflow replaces exactly one stale `AppConfig`
comment that described missing introspection credentials as backward-compatible
open access. Runtime behavior is already fail-closed. The replacement documents
that missing, malformed, duplicate, coalesced, or incorrect service credentials
are rejected and that absence of `AUTH_INTROSPECT_SECRET` disables
introspection.

The workflow refuses to continue unless the starting pull-request head is exact,
the old block occurs exactly once, only `src/config.rs` changes, the diff passes
`git diff --check`, and publication is a normal non-force fast-forward to the
existing pull-request branch.

## Credential boundary

The trusted job generates an ephemeral RSA-3072 key and posts its public key as
a one-time private challenge. The owner replies with an
RSA-OAEP-SHA256/MGF1-SHA256 ciphertext. The decrypted token is masked and used
only to:

1. verify the authenticated account is `ORESoftware`;
2. verify private pull and push access to the exact target;
3. fetch the exact starting commit;
4. publish the deterministic documentation-only fast-forward;
5. verify the pull request now points to the resulting commit.

Before any repository code, build script, dependency, test, browser, audit, or
container build executes, the job removes the Git remote and clears the
plaintext token, askpass helper, RSA material, ciphertext, and GitHub credential
environment variables. The token is never committed, printed, placed in an
Actions output or environment file, or uploaded as an artifact.

## Workspace provenance

Every path the matrix touches belongs to the target repository, not to this
host. The trusted job empties `$GITHUB_WORKSPACE` and replaces it with a shallow
clone of `shared-auth/shared-auth-server.rs` at the exact starting commit, so
`src/config.rs`, `db/schema.sql`, `Cargo.lock`, `Dockerfile`, and the `e2e/`
Playwright suite are resolved against that clone. This host repository has no
`e2e/` directory and is not meant to have one; a manifest scanner that resolves
`cd e2e && npm ci` against this repository will report a false positive.

Immediately after checkout — and deliberately before the deterministic
documentation correction and before the one-time owner credential is spent on a
fast-forward push — the job asserts that each of those paths exists and that the
browser suite still declares an `npm test` script. A moved or deleted suite
therefore fails fast, by name, while the run is still reversible, rather than as
an unexplained missing-directory error after the target pull request has already
been advanced.

## Executable matrix

The resulting exact pull-request head is subjected to:

- `cargo fmt --all --check`;
- declarative PostgreSQL schema application;
- `cargo clippy --all-targets --locked -- -D warnings`;
- `cargo test --all-targets --locked`;
- focused authenticated, inactive-token, missing-secret, malformed-credential,
  duplicate-header, coalesced-header, case-insensitive bearer, and MFA freshness
  tests;
- a locked release build;
- real Chromium WebAuthn ceremonies backed by PostgreSQL and Redis;
- the pinned RustSec audit action;
- a Docker build without publication.

On success, the workflow reports the resulting exact SHA and closes the
metadata-only carrier without merging it. After Shared Auth pull request 30 is
merged, remove this temporary validator through a separate reviewed cleanup
pull request.
