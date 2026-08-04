# Project automation webhook

Tracked by Linear `DEN-1906` and GitHub issue `ORESoftware/k8s-cluster#861`.

This Argo CD workload receives signed GitHub App webhooks and applies the canonical
GitHub Project #1 and Linear lifecycle policy from `ORESoftware/project-registry`.
The source implementation merged in `ORESoftware/project-registry#14` at
`d31d817cee22cc5cd6d4473a239012f38d16fbe2`; the deployment pins that immutable
image rather than a moving tag.

It is deliberately single-replica because the initial delivery de-duplication store is
process-local. Do not scale horizontally until the service uses a durable shared store.

## Required secret-store records

Create these records in the backing store used by `dd-cluster-secrets` before syncing:

- `dd/remote-dev/project-automation`
  - `GITHUB_APP_ID`
  - `GITHUB_APP_PRIVATE_KEY`
  - `GITHUB_WEBHOOK_SECRET`
  - `LINEAR_API_KEY`
- `dd/remote-dev/project-automation-ghcr-pull`
  - `dockerconfigjson`

No secret values belong in Git.

## GitHub App configuration

The dedicated App must be installed on all 44 mapped organizations, subscribe to
`issues` and `pull_request`, and have Metadata read, Issues write, Pull requests read,
and Organization projects write permissions. Configure its webhook as:

`https://project-automation.95-217-171-250.sslip.io/webhooks/github`

Do not enable delivery until the certificate is valid, the immutable image is published,
the ExternalSecrets are Ready, and `/healthz` returns 200.

## Canary and promotion

1. Confirm the pinned `d31d817cee22cc5cd6d4473a239012f38d16fbe2` image exists in GHCR.
2. Sync this Argo CD Application and verify one Ready pod only.
3. Open a test issue in a non-critical organization repository and verify its Linear
   issue, Project item, and Backlog status.
4. Open a PR containing a `DEN-123` reference and `Closes #123`; verify In Progress.
5. Merge a canary PR to `integration`; verify the distinct integration state.
6. Merge the release PR to `main` or `master`; verify Done in GitHub and Linear.
7. Review GitHub webhook delivery logs before enabling the App fleet-wide.
