# GitHub App write verification

Linear issue: DEN-43

Verified on 2026-07-27 against `ORESoftware/k8s-cluster` using the connected GitHub App.

## Minimum effective repository permissions

- **Metadata:** Read
- **Contents:** Read and write
- **Pull requests:** Read and write
- **Commit statuses and checks:** Read when CI evidence must be inspected

Repository installation scope must include `ORESoftware/k8s-cluster`. Repository-level permission metadata alone is not sufficient when the app is not installed for the selected repository or account.

## Verified path

1. Created `agent/den-43-github-app-write-verification` from `main` through the connected GitHub App.
2. Created a harmless documentation commit through the GitHub Contents API.
3. Opened GitHub pull request #41 targeting `main` with `Fixes DEN-43`.
4. Confirmed Linear attached the pull request to DEN-43 and exposed the GitHub backlink.
5. Continued using the same connected App to create and merge subsequent feature branches and pull requests in this repository.
6. Rebuilt this documentation-only change from current `main` after the original verification branch became stale; no force-push or rebase was used.

The shared repository checks may still report unrelated private-submodule installation gaps tracked by DEN-255 and DEN-370. Those failures do not invalidate the write-path verification: branch, commit, pull-request, comment, check-inspection, and merge mutations have all succeeded through the connected App.
## Verification path

1. Cut `agent/den-43-github-app-write-verification` from `main`.
2. Create this harmless documentation commit through the GitHub contents API.
3. Open a draft pull request targeting `main` with `Fixes DEN-43`.
4. Confirm GitHub Actions completes successfully.
5. Confirm Linear attaches the branch, commit, and pull request and GitHub displays the Linear backlink.
6. Merge only after the complete path is verified.

This file intentionally contains no credentials, tokens, secrets, personal data, or runtime configuration changes.
