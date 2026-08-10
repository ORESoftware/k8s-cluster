# Shared Auth web-server repository provisioning

This runbook governs the one-time creation of the private repository
`shared-auth/shared-auth-web-server.js` from the exact corrected artifact certified in
`shared-auth-test/contract-conformance-tests#6`.

The workflow is deliberately narrower than the general fleet publisher. It cannot select a
different organization, repository, canary, workflow run, artifact, seed commit, or repair.
It does not consume a user-supplied token on a GitHub-hosted runner.

## Reviewed evidence

| Item | Pinned value |
| --- | --- |
| Test repository | `shared-auth-test/contract-conformance-tests` |
| Pull request | `#6` |
| Tested head | `252cbda966081d902637fded7adf51d949b919cd` |
| Successful workflow run | `31442332258` |
| Candidate artifact | `shared-auth-web-server-candidate-v1` |
| Artifact ID | `5866515977` |
| Original seed commit | `67a3b5138a4050a23a409094ef094b050bb162fd` |
| Original archive SHA-256 | `095c5e0c464aae73b85f399614c0ad11be1acfb67fd2a40a4da4ee1da83cc848` |
| Reviewed repair | remove the duplicate plain `axum = "0.7"` declaration |

The successful canary reconstructed and verified all 38 seed files, ran repository policy and
language-syntax checks, generated a `Cargo.lock`, compiled and tested default and all-feature
Rust targets, and ran Clippy with warnings denied. It then generated a deterministic candidate
archive with a manifest containing archive, lockfile, and per-file hashes.

## Safety properties

`scripts/ops/provision_shared_auth_web_server.py` refuses to proceed unless all of the
following are true:

- the request selects only `shared-auth/shared-auth-web-server.js` and keeps it private;
- the pinned canary pull request is merged;
- the pinned workflow run completed successfully for the exact tested head;
- the exact artifact ID and name exist and are not expired;
- artifact redirects remain HTTPS and credentials are stripped on cross-host redirects;
- ZIP and tar entries are regular, bounded, unique, and free of traversal or symlink paths;
- the candidate archive, `Cargo.lock`, and every file match the artifact manifest;
- the reviewed duplicate-Axum repair is present;
- no broad secret, private-key, or raw-biometric marker appears in the source tree;
- an existing `main` branch is either byte-for-byte identical or publication stops;
- no force update is used.

After publication, the script verifies the exact Git blob tree and applies a protected `main`
branch with review, stale-review dismissal, last-push approval, linear history, conversation
resolution, admin enforcement, and force-push/deletion denial.

## Inert review phase

`ops/requests/shared-auth-web-server.json` is committed with:

```json
"execute": false
```

Pull-request and ordinary `main` validation therefore run only the parser and offline boundary
tests. No repository is created, no artifact is downloaded with privileged credentials, and
no GitHub organization state is changed.

## Activation phase

Activation is a separate, reviewable one-line change from `false` to `true`. Merge that change
only after the inert implementation PR is green and approved.

On the resulting `main` push, the workflow:

1. validates and hashes the exact request and provisioner on a GitHub-hosted runner;
2. obtains an AWS role through GitHub OIDC;
3. sends an exact-commit command to the protected operations host through SSM;
4. re-checks the script and request hashes on that host;
5. retrieves `/oresoftware/github-token` from encrypted SSM Parameter Store only on the host;
6. verifies the pinned GitHub canary and downloads the pinned candidate artifact;
7. creates or verifies the one allowed private repository;
8. publishes the exact candidate tree and applies hardening;
9. prints a non-secret verification record containing the repository, commit, candidate digest,
   lockfile digest, file count, privacy/default-branch state, and branch-protection result.

Required repository variables:

- `ORE_K8S_OPS_AWS_ROLE_ARN`
- `ORE_K8S_OPS_INSTANCE_ID`

Required protected-host parameter:

- `/oresoftware/github-token`

The token must be organization-scoped and limited to the repository-administration and contents
operations required by this one-time provisioner. Rotate it after the provisioning record is
captured.

## Failure and recovery

The operation is designed to be idempotent. Re-running against an absent or exact-matching
repository is safe. A divergent `main` tree is a hard stop and requires human review; do not
force-push, delete the branch, or weaken the digest checks.

If GitHub Actions rejects the workflow before runner allocation because of billing or spending
limits, that is not test evidence. Restore runner admission or run the already-reviewed exact
commit through the approved self-hosted operations path, preserving the same request/script
hashes and canary pins.

If the target repository is created but a later settings call fails, inspect the sanitized SSM
result, verify the exact tree first, then rerun the same request. Do not recreate the repository
or substitute another artifact.
