# GitHub Project and Linear lifecycle automation

This Argo-managed package contains two independent automation surfaces:

- the canonical DEN-1906 GitHub/Linear lifecycle webhook from `ORESoftware/project-registry`;
- the active DEN-2745 nightly fleet interdependency scheduler.

## Webhook current state

The webhook Deployment is intentionally committed with `replicas: 0`. Argo can
own and self-heal the manifests without accepting webhook traffic until every
activation prerequisite is verified. Do not scale it with an imperative
`kubectl` command; promote `replicas` from 0 to 1 through a reviewed pull request.

Immutable webhook image:

`ghcr.io/oresoftware/project-automation:d31d817cee22cc5cd6d4473a239012f38d16fbe2`

Public callback:

`https://project-automation.95-217-171-250.sslip.io/webhooks/github`

Health endpoint:

`https://project-automation.95-217-171-250.sslip.io/healthz`

## Nightly interdependency scheduler

`dd-nightly-interdependency` runs at `02:00` in `America/Chicago`. Kubernetes
owns daylight-saving-time conversion through `spec.timeZone`; the schedule is
not approximated with a fixed UTC cron.

The scheduler is independent of the dormant webhook replica count. Each run:

1. verifies and fetches the exact immutable `ORESoftware/project-registry`
   source revision declared in the CronJob;
2. dispatches one fail-closed fleet graph preflight that inventories
   `.gitmodules`/gitlinks, Nix manifests and locks, `.zpkg.toml`, and `.zpkg.lock`;
3. requires the graph phase to publish deterministic JSON, DOT, Mermaid, and
   Markdown artifacts with SCCs and topological update waves;
4. only after a successful graph result, dispatches the bounded organization
   update units with paired `-test` organization context;
5. records focused update PRs, exact-marker supersession decisions, and
   deduplicated Linear blockers under DEN-2745.

The CronJob forbids overlap, has a 12-hour fleet deadline, runs at most six
organization units concurrently, uses a read-only GitHub App token rather than a
PAT, and can reach only DNS, GitHub over HTTPS, and the internal coordinator on
TCP 8080.

## Secret contract

Create `dd/remote-dev/project-automation-secrets` in the provider backing the
`dd-cluster-secrets` ClusterSecretStore with these JSON properties:

- `github_app_id`
- `github_app_private_key`
- `github_webhook_secret`
- `linear_api_key`

Create `dd/remote-dev/ai-agent-coordinator-secrets` with:

- `COORDINATOR_API_TOKEN`

The ExternalSecret projects the coordinator value into the workload Secret as
`coordinator_api_token`. The GitHub credential is an App private key. A personal
access token is not an accepted substitute.

## GitHub App contract

Install the App on every managed organization and subscribe it to `issues` and
`pull_request` events. Required permissions for the webhook are:

- repository metadata: read
- issues: read and write
- pull requests: read
- organization projects: read and write

The nightly bootstrap initially mints a repository-scoped, contents-read token
for `ORESoftware/project-registry`. The dispatched agent work must use the
portfolio App's governed organization permissions and the exact organization
mapping carried in the coordinator payload.

Configure the callback URL above and use the same high-entropy secret stored as
`github_webhook_secret`.

## Container pull boundary

GHCR packages linked to private repositories are private by default. Before the
webhook activation PR changes the replica count, choose and verify one of these
explicit paths:

1. Make the package public and use anonymous pulls. This is an irreversible
   visibility decision and must be reviewed separately.
2. Mirror the immutable image into a registry the cluster can pull through
   workload identity or another non-personal credential.

The nightly scheduler avoids this unresolved boundary by using an immutable
official Node image and fetching only the exact reviewed project-registry source
commit with a short-lived GitHub App installation token.

Do not add a classic GitHub PAT merely to make either pod start.

## Project preflight

For each of the 44 organizations, verify Project #1 has a single-select `Status`
field with compatible options for:

- `Todo` or `Backlog`
- `In Progress`
- `Integration`, `Integrated`, or `Ready for production`
- `Done`

The service fails closed when an owner, project, status field, or status option is
missing or ambiguous.

## Webhook canary sequence

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

Webhook rollback is a reviewed change restoring `replicas: 0`. Nightly scheduler
rollback is a reviewed change setting the CronJob `suspend` field to `true` or
reverting the scheduler resources.