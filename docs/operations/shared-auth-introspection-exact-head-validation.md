# Shared Auth introspection exact-head validation

`shared-auth/shared-auth-server.rs` PR #30 carries the canonical authenticated
introspection and MFA `auth_time` contract. The Shared Auth organization currently
cannot allocate a GitHub-hosted runner: every exact-head job terminates before step
one and exposes no job log. This temporary trusted-main workflow provides an
independent executable result without treating a platform pre-runner failure as a
Rust result.

## Exact target

The workflow is deliberately pinned to all of the following:

- repository: `shared-auth/shared-auth-server.rs`;
- pull request: `30`;
- branch: `agent/shared-auth-auth-time-claim`;
- commit: `c34015d337548b272d4e7fdb433ce402b6b3ed55`.

A metadata-only carrier is accepted only when it is an owner-created, same-repo,
draft, one-commit pull request with one exact six-line marker. Carrier code is
never checked out or executed.

## Credential boundary

The trusted `pull_request_target` job generates an ephemeral RSA-3072 key and posts
the public key as a one-time challenge. The owner replies with an
RSA-OAEP-SHA256/MGF1-SHA256 ciphertext. The decrypted token is masked and used only
to verify the owner, inspect the exact private pull request, and fetch its exact
commit.

Before any repository code, dependency, build script, or test executes, the job:

1. removes the remote;
2. clears the plaintext token and GitHub credential environment variables;
3. deletes the askpass helper, private key, public key, and ciphertext;
4. confirms the detached checkout matches the pinned SHA.

No plaintext credential is committed, printed, placed in an Actions output or
environment file, or uploaded as an artifact.

## Executable validation

The exact private commit is subjected to:

- formatting;
- declarative PostgreSQL schema application;
- Clippy with warnings denied;
- the complete locked all-target test graph;
- focused authenticated, inactive-token, missing-secret, malformed-credential,
  duplicate-header, and MFA freshness tests;
- a locked release build;
- real Chromium WebAuthn ceremonies;
- a Docker build without publication.

The workflow posts a bounded failure tail or a success manifest to the carrier.
The carrier is closed, never merged, after success. Remove the temporary trusted
workflow after PR #30 is certified and merged.
