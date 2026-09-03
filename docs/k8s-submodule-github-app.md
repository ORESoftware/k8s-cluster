# Cross-organization GitHub App for pinned CI submodules

Linear: DEN-255, DEN-370, DEN-1095, DEN-1537

`repo checks` verifies source and contracts across every repository pinned under
`remote/deployments`. The Scintilla native amd64/arm64 image benchmark also
needs the exact private runner source while building from the authoritative
cluster-root context. CI obtains short-lived, owner-scoped GitHub App
installation tokens; it must not use a long-lived cross-organization PAT.

This document is the configuration and recovery runbook for the
`backend pins + private deployment contracts` job and for trusted
full-superproject build proofs. Those jobs are expected to fail closed before
private source tests when the App ID, private key, installation, repository
selection, pinned checkout, or returned token scope is unavailable.

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
intentional allowlist review in the same pull request. The allowlist includes
both `scintilla-run/gleam-lambda-runner` and
`scintilla-run/scintilla-run-monorepo`; do not create a separate broader App or
fall back to a personal token for those repositories.

## Installation procedure

For each owner in the allowlist:

1. Install the same dedicated App on that account or organization.
2. Select **only** the repositories listed for that owner.
3. Complete any required organization-owner approval or SAML/SSO authorization.
4. Record the responsible organization owner and App owner in the approved
   operations inventory outside source control.
5. Do not enable access to all current or future repositories.

A token request is restricted to one owner installation and to the exact
repository names selected from the allowlist. A failure in one owner does not
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

The trusted-main bootstrap discovers candidate App credentials only through the
protected administration host. Before it writes either Actions secret, it now
validates the **same App ID/private-key pair against every repository in the
authoritative allowlist**. For each repository it mints a separate token limited
to that one repository, verifies the returned repository inventory and
permissions, and revokes the validation token. A pair that works for only the
library repository or only the Scintilla organization is rejected.

The workflow generates a signed App JWT in memory, requests tokens for exact
repository selections, and validates GitHub's response before creating the
secret output files. Validation requires:

- a bounded printable single-line token and a non-empty expiration;
- `contents:read`, with only GitHub's implicit `metadata:read` also permitted;
- a returned repository list that exactly equals the requested single
  owner/repository pair;
- no duplicate requested or returned repository names;
- successful validation for the complete allowlist using one credential pair;
- revocation of every validation token before the next phase.

Successful token creation alone is not authorization evidence. The returned
permission and repository metadata are part of the authorization decision and
must satisfy every exact-scope check before the App credentials become usable.

Missing repository proof, substituted repositories, duplicate repositories, a
partial cross-organization installation, or any broader permission fails closed.
The selected App ID and private key are written to mode-`0600` temporary files
only after all checks pass. Runtime checkout tokens also expire automatically
and are revoked by their caller after use.

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
   only;
5. confirm tokens are minted for one owner/repository selection at a time and
   are revoked by the job cleanup path.

The bootstrap workflow itself runs only from trusted `main`. A feature branch
must not gain a secret-hydration carrier merely to make its own checks green.
Merge reviewed bootstrap changes first, inspect the trusted-main evidence, and
then rerun dependent pull requests.

## Configuration verification

After App installations and repository secrets are configured:

1. Confirm the App permissions are still exactly Metadata read and Contents read.
2. Confirm every owner installation selects only the repositories in the
   authoritative allowlist.
3. Confirm the bootstrap evidence uses schema version 2, lists the exact sorted
   allowlist, and contains one repository-restricted installation proof per
   entry.
4. Confirm the token-mint regression test passes its permission-drift,
   repository-substitution, missing-scope, partial-installation, and duplicate-
   name cases.
5. Confirm the two repository Actions secret names exist without reading or
   exporting their values.
6. Re-run the failed `backend pins + private deployment contracts` job and the
   Scintilla native image benchmark on their current authoritative commits.
7. Download sanitized access and benchmark reports and confirm every expected
   private checkout actually ran; a skipped test is not a green result.
8. Confirm every checkout commit equals its superproject gitlink or the explicit
   immutable candidate SHA named by the benchmark.
9. Confirm reports contain no token, key, credential-bearing URL, subject, email,
   or private payload.
10. Attach only the App name/ID, installation owners, repository scope, workflow
    run, commit, sanitized report digest, and generic outcome to the associated
    Linear issues.

Do not rely on historic PR numbers or a green run for an older head. The evidence
must name the current authoritative commit.

The scripts fail closed when credentials are missing, an installation cannot be
resolved, a repository is absent from the allowlist, a token request is broader
than the selected repository, GitHub returns broader permissions or a different
repository set, only a subset of the allowlist validates, or a checkout differs
from its pinned commit.

## Current recovery procedure (DEN-1537 and DEN-1095)

When a job reports that `K8S_SUBMODULE_APP_ID` or
`K8S_SUBMODULE_APP_PRIVATE_KEY` is required:

1. Leave the required check red. Do not skip the job or change it to advisory.
2. Determine whether the dedicated App still exists and whether its private key
   was revoked, expired by policy, or never added to repository Actions secrets.
3. Verify the App installations against the current allowlist, including both
   Scintilla repositories.
4. Create or rotate a private key in the GitHub App settings only through the
   approved owner process.
5. Run the trusted-main
   `Bootstrap k8s submodule GitHub App secrets` workflow. It must validate one
   credential pair across the whole allowlist before replacing the two
   repository secrets.
6. Re-run only the failed private-checkout or benchmark job first; then require
   the complete workflow on the same commit.
7. Inspect exact-scope token tests, sanitized evidence, and private-source test
   steps before resolving the Linear issue.

A personal access token supplied in chat, a developer shell, or an ad-hoc secret
is not an acceptable recovery mechanism.

## Rotation and incident response

Rotate the App private key at least every 90 days and immediately after suspected
exposure. Add the new private key in GitHub, replace the Actions secret through
the trusted bootstrap, verify full repository-check and Scintilla benchmark
runs, then revoke the old key. Rotation does not require changing the App ID or
installation selections.

If an installation token or private key may have leaked:

1. Cancel active workflow runs.
2. Revoke the affected App private key in GitHub.
3. Review App and organization audit logs.
4. Rotate the protected source and Actions secret.
5. Re-run the sanitized access report before allowing dependent merges.

Do not fall back to an exposed or broadly scoped PAT. During an outage, leave the
fleet-wide contract and full-superproject benchmark red rather than silently
reducing repository coverage.

## Rollback

The code-only rollback is to revert the GitHub App helper changes and restore the
previous workflow, but that also restores its single-repository validation gap.
Prefer fixing App installations or protected credentials. Never bypass the
repository contracts or replace failed checkouts with branch heads; all
submodules must remain at their immutable superproject gitlinks.
