# GitHub Project and Linear lifecycle webhook

This Argo-managed service deploys the canonical implementation from
`ORESoftware/project-registry` for DEN-1906.

## Current state

The Deployment is intentionally committed with `replicas: 0`. Argo can own and
self-heal the manifests without accepting webhook traffic until every activation
prerequisite is verified. Do not scale it with an imperative `kubectl` command;
promote `replicas` from 0 to 1 through a reviewed pull request.

Immutable image:

`ghcr.io/oresoftware/project-automation:d31d817cee22cc5cd6d4473a239012f38d16fbe2`

Public callback:

`https://project-automation.95-217-171-250.sslip.io/webhooks/github`

Health endpoint:

`https://project-automation.95-217-171-250.sslip.io/healthz`

## Secret contract

Create `dd/remote-dev/project-automation-secrets` in the provider backing the
`dd-cluster-secrets` ClusterSecretStore with these JSON properties:

- `github_app_id`
- `github_app_private_key`
- `github_webhook_secret`
- `linear_api_key`

The GitHub credential is an App private key. A personal access token is not an
accepted substitute.

## GitHub App contract

Install the App on every managed organization and subscribe it to `issues` and
`pull_request` events. Required permissions are:

- repository metadata: read
- issues: read and write
- pull requests: read
- organization projects: read and write

Configure the callback URL above and use the same high-entropy secret stored as
`github_webhook_secret`.

## Container pull boundary

GHCR packages linked to private repositories are private by default. Before the
activation PR changes the replica count, choose and verify one of these explicit
paths:

1. Make the package public and use anonymous pulls. This is an irreversible
   visibility decision and must be reviewed separately.
2. Mirror the immutable image into a registry the cluster can pull through
   workload identity or another non-personal credential.

Do not add a classic GitHub PAT merely to make the pod start.

## Project preflight

For each of the 44 organizations, verify Project #1 has a single-select `Status`
field with compatible options for:

- `Todo` or `Backlog`
- `In Progress`
- `Integration`, `Integrated`, or `Ready for production`
- `Done`

The service fails closed when an owner, project, status field, or status option is
missing or ambiguous.

## Canary sequence

1. Create a GitHub issue in one canary repository. Confirm its Project #1 item is
   `Todo` and that a mapped Linear issue is created in `Backlog`.
2. Open a PR whose branch/title/body contains the Linear identifier and whose body
   contains `Closes #<issue>`. Confirm both cards move to `In Progress`.
3. Merge the PR to `integration`. Confirm GitHub Project status is `Integration`
   and Linear status is `In Review`.
4. Promote the integration commit to `main` or `master`. Confirm both systems move
   to `Done`.
5. Repeat the production transition once with the same delivery ID in a controlled
   test to verify idempotency.

Rollback is a reviewed change restoring `replicas: 0`.
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
