# Cross-organization GitHub App for pinned CI submodules

Linear: DEN-255, DEN-370, DEN-1537

`repo checks` verifies source and contracts across every repository pinned under
`remote/deployments`. CI obtains short-lived, owner-scoped GitHub App installation
tokens; it must not use a long-lived cross-organization PAT.

This document is the configuration and recovery runbook for the
`backend pins + private deployment contracts` job. The job is expected to fail
closed before private source tests when the App ID, private key, installation,
repository selection, pinned checkout, or returned token scope is unavailable.

## Permission boundary

Create one dedicated GitHub App whose repository permissions are exactly:

| Permission | Access |
| --- | --- |
| Metadata | Read |
| Contents | Read |

Do not grant pull-request, issue, Actions, workflow, administration, secret,
package-write, environment, deployment, organization-write, or user permissions.
The App does not push code. The normal workflow `GITHUB_TOKEN` remains read-only.

The authoritative repository set is
[`config/ci/k8s-submodule-github-app-allowlist.json`](../config/ci/k8s-submodule-github-app-allowlist.json).
Static CI requires that list to match the `remote/deployments` gitlinks exactly.
Adding, removing, or renaming a deployment submodule therefore requires an
intentional allowlist review in the same pull request.

## Installation procedure

For each owner in the allowlist:

1. Install the same dedicated App on that account or organization.
2. Select **only** the repositories listed for that owner.
3. Complete any required organization-owner approval or SAML/SSO authorization.
4. Record the responsible organization owner and App owner in the approved
   operations inventory outside source control.
5. Do not enable access to all current or future repositories.

A token request is restricted to one owner installation and to the exact
repository names selected from `.gitmodules`. A failure in one owner does not
cause credentials from another owner to be reused.

## GitHub Actions secrets

Store these repository-level Actions secrets on `ORESoftware/k8s-cluster`:

- `K8S_SUBMODULE_APP_ID` — the numeric App ID; not an installation ID.
- `K8S_SUBMODULE_APP_PRIVATE_KEY` — the complete PEM private key, including
  header and footer lines.

Never put either value in Linear, chat, commits, pull-request text, workflow
arguments, artifacts, diagnostic reports, shell history, screenshots, or support
attachments. Do not print, partially mask, hash, count, or base64-encode the PEM
as evidence.

The workflow generates a signed App JWT in memory, requests a token for the exact
owner-local repository list, and validates GitHub's response before creating the
token output file. Validation requires:

- a bounded printable single-line token and a non-empty expiration;
- `contents:read`, with only GitHub's implicit `metadata:read` also permitted;
- a returned repository list that exactly equals the requested owner/repository
  set, independent of response order;
- no duplicate requested or returned repository names.

Missing repository proof, substituted repositories, duplicate repositories, or
any broader permission fail closed. The token is written to a mode-`0600` temporary file
only after those checks pass and is revoked after the corresponding owner batch.
Tokens also expire automatically.

## Trusted pull-request boundary

Repository Actions secrets must be available only to trusted runs under the
repository's existing security policy. Do not expose the App private key to code
from an untrusted fork or to an unreviewed workflow change.

Before approving a workflow that can read these secrets:

1. review the exact workflow and helper diff;
2. confirm the run checks out the trusted merge candidate rather than attacker-
   controlled workflow code;
3. confirm no step prints environment variables, enables `set -x`, uploads raw
   temporary files, or embeds credentials in Git URLs;
4. confirm the sanitized report contains repository metadata and commit state
   only.

## Configuration verification

After App installations and repository secrets are configured:

1. Confirm the App permissions are still exactly Metadata read and Contents read.
2. Confirm every owner installation selects only the repositories in the
   authoritative allowlist.
3. Confirm the token-mint regression test passes its permission-drift,
   repository-substitution, missing-scope, and duplicate-name cases.
4. Confirm the two repository Actions secret names exist without reading or
   exporting their values.
5. Re-run the failed `backend pins + private deployment contracts` job on the
   current authoritative pull request and commit.
6. Download `backend-submodule-access-report` and confirm every allowlisted row is
   `success`.
7. Confirm every checkout commit equals its superproject gitlink.
8. Confirm the report has no token, key, credential-bearing URL, subject, email,
   or private payload.
9. Confirm the private deployment architecture and source contract tests actually
   ran after checkout; a skipped test is not a green configuration result.
10. Attach only the App name/ID, installation owners, repository scope, workflow
    run, commit, sanitized report digest, and generic outcome to DEN-1537.

Do not rely on historic PR numbers or a green run for an older head. The evidence
must name the current authoritative commit.

The scripts fail closed when credentials are missing, an installation cannot be
resolved, a repository is absent from the allowlist, a token request is broader
than the selected repository set, GitHub returns broader permissions or a
different repository set, or a checkout differs from its pinned commit.

## Current recovery procedure (DEN-1537)

When the job reports that `K8S_SUBMODULE_APP_ID` or
`K8S_SUBMODULE_APP_PRIVATE_KEY` is required:

1. Leave the required check red. Do not skip the job or change it to advisory.
2. Determine whether the dedicated App still exists and whether its private key
   was revoked, expired by policy, or never added to repository Actions secrets.
3. Verify the App installations against the current allowlist.
4. Create or rotate a private key in the GitHub App settings.
5. Replace the two repository Actions secrets using the approved secret-management
   path. Never paste their values into DEN-1537 or a pull-request comment.
6. Re-run only the failed job first; then require the complete `repo checks`
   workflow on the same commit.
7. Inspect the exact-scope token test, sanitized report, and private-source test
   steps before resolving DEN-1537.

A personal access token supplied in chat, a developer shell, or an ad-hoc secret
is not an acceptable recovery mechanism.

## Rotation and incident response

Rotate the App private key at least every 90 days and immediately after suspected
exposure. Add the new private key in GitHub, replace the Actions secret, verify a
full repository-check run, then revoke the old key. Rotation does not require
changing the App ID or installation selections.

If an installation token or private key may have leaked:

1. Cancel active workflow runs.
2. Revoke the affected App private key in GitHub.
3. Review App and organization audit logs.
4. Rotate the Actions secret.
5. Re-run the sanitized access report before allowing dependent merges.

Do not fall back to an exposed or broadly scoped PAT. During an outage, leave the
fleet-wide contract job red rather than silently reducing repository coverage.

## Rollback

The code-only rollback is to revert the GitHub App helper changes and restore the
previous workflow, but that also restores its credential and observability
limitations. Prefer fixing App installations or secrets. Never bypass the
repository contracts or replace failed checkouts with branch heads; all
submodules must remain at their immutable superproject gitlinks.
