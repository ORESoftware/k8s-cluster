# Cross-organization GitHub App for pinned CI submodules

Linear: DEN-255, DEN-370

`repo checks` verifies source and contracts across every repository pinned under
`remote/deployments`. CI obtains short-lived, owner-scoped GitHub App installation
tokens; it must not use a long-lived cross-organization PAT.

## Permission boundary

Create a dedicated GitHub App whose repository permissions are exactly:

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
4. Record the responsible organization owner outside source control.
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
arguments, artifacts, or diagnostic reports. The workflow generates a signed App
JWT in memory, mints repository-restricted installation tokens, writes each token
to a mode-`0600` temporary file, and revokes it after the corresponding owner
batch. Tokens also expire automatically.

## Verification

After installations and secrets are configured:

1. Re-run `repo checks` on PR #47.
2. Download `backend-submodule-access-report` and confirm every row is `success`.
3. Confirm each checkout commit equals the superproject gitlink.
4. Confirm the report has no token, key, credential-bearing URL, subject, email,
   or private payload.
5. Re-run PR #41 and PR #42; do not merge them until their required checks pass.

The scripts fail closed when credentials are missing, an installation cannot be
resolved, a repository is absent from the allowlist, a token request is broader
than the selected repository set, or a checkout differs from its pinned commit.

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
