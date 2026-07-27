# GitHub App write verification

Linear issue: DEN-43

Verified on 2026-07-27 against `ORESoftware/k8s-cluster` using the connected GitHub App.

## Minimum effective repository permissions

- **Metadata:** Read
- **Contents:** Read and write
- **Pull requests:** Read and write
- **Commit statuses and checks:** Read when CI evidence must be inspected

Repository installation scope must include `ORESoftware/k8s-cluster`. Repository-level permission metadata alone is not sufficient when the app is not installed for the selected repository or account.

## Verification path

1. Cut `agent/den-43-github-app-write-verification` from `main`.
2. Create this harmless documentation commit through the GitHub contents API.
3. Open a draft pull request targeting `main` with `Fixes DEN-43`.
4. Confirm GitHub Actions completes successfully.
5. Confirm Linear attaches the branch, commit, and pull request and GitHub displays the Linear backlink.
6. Merge only after the complete path is verified.

This file intentionally contains no credentials, tokens, secrets, personal data, or runtime configuration changes.
