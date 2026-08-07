# Canonical Docs ephemeral credential publication

This runbook documents the one-time path used to create
`canonical-cloud/canonical-docs` without committing, logging, or storing a
repository-administration credential in GitHub Actions.

## Boundaries

- The durable source is a credential-free Git bundle split into four reviewed
  base64 assets in `scripts/critical-org-fleet/assets/`.
- The bundle contains exactly the preserved initial `main` commit and one
  review branch. Existing or additional refs fail verification.
- The `pull_request_target` broker executes only from trusted `main`. The
  carrier is a one-commit, one-file, four-line metadata trigger and is never
  merged.
- Each run generates a fresh 3072-bit RSA key pair. Only the public key is
  posted to the carrier. The private key exists only in the ephemeral runner
  and is destroyed during cleanup.
- The operator response is RSA-OAEP with SHA-256 for both OAEP and MGF1. The
  plaintext token is decrypted in memory, masked, and never written to Git,
  Actions outputs, environment files, artifacts, or command arguments.
- The token must authenticate as `ORESoftware` with active administrator
  membership in `canonical-cloud`. The publisher allowlist contains only the
  exact public repository `canonical-cloud/canonical-docs`.
- Repository creation is idempotent. Existing refs are accepted only when they
  equal the reviewed SHA; no force update or history replacement is available.
- The publisher creates the review PR but does not merge it. Normal repository
  CI, review, exact-head certification, and merge policy remain mandatory.

## Reviewed source

| Item | Value |
| --- | --- |
| Bundle SHA-256 | `3169c190a11f8889ca0a29d5db58acabae1e3b887cc302407ccc350d3a461828` |
| Initial `main` | `1848835599049ca41f68a079b5ac04f7d360fe87` |
| Review branch | `agent/den-1049-repository-baseline` |
| Review head | `54aa2efcbcfd21020614cbecccea5a907ead813f` |
| Business plan SHA-256 | `b3bfd4d8596adffd3ed93ef3f530c46c5710f2ed6e6b9bff2929943628c22fe7` |

## Carrier contract

The carrier marker is exactly:

```text
target=canonical-cloud/canonical-docs
trusted-main=<current k8s-cluster main commit>
bundle-sha256=3169c190a11f8889ca0a29d5db58acabae1e3b887cc302407ccc350d3a461828
protocol=rsa-oaep-sha256-v1
```

The carrier title is exactly:

```text
DO NOT MERGE: publish canonical-docs with encrypted credential
```

Close the carrier without merge after it records the created repository and
target PR. The target PR is then reviewed and merged through the ordinary
Canonical Cloud workflow.

Refs DEN-1049, DEN-319, DEN-621, and DEN-127.
