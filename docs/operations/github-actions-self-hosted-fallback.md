# GitHub Actions self-hosted fallback: AWS + Hetzner

**Status date:** 2026-08-03  
**Repository:** `ORESoftware/k8s-cluster`  
**Execution label:** `oresoftware-ci`

## Decision

Use GitHub's official Actions Runner Controller (ARC) for GitHub Actions
semantics, and use the existing `dd-build-server` for fixed operator-reviewed
profiles. Do not build a second interpreter for workflow YAML.

`gha-clone-server-rs` is deliberately smaller than a CI engine. It verifies a
GitHub `workflow_run` webhook, matches an exact allowlist, then does one of two
things:

1. dispatches a named repository workflow whose jobs target ARC; or
2. submits a named, already-installed `dd-build-server` profile.

It cannot accept a shell command, container image, repository URL, workflow
file, runner label, or build profile from the webhook payload.

## Current hosted-Actions observation

On 2026-08-03, recent `ORESoftware/k8s-cluster` commits still had GitHub-hosted
workflow runs entering `in_progress` and completing successfully. That proves
the organization was not completely blocked from starting Actions at that
moment. It does **not** reveal the exact remaining paid-minute or spending-limit
balance.

An organization owner or billing manager can query enhanced billing usage with:

```bash
GH_TOKEN='read-only-org-admin-token' \
gh api \
  -H 'Accept: application/vnd.github+json' \
  -H 'X-GitHub-Api-Version: 2026-03-10' \
  /organizations/ORESoftware/settings/billing/usage
```

Run this with a newly issued least-privilege credential. Never reuse a token
that has appeared in chat, logs, shell history, workflow input, or a committed
file.

## Architecture

```text
GitHub organization workflow_run webhook
              |
              | HTTPS + X-Hub-Signature-256
              v
Hetzner ingress /ci/github/webhook
              |
              v
gha-clone-server-rs
       |                         |
       | workflow_dispatch       | POST /builds
       v                         v
GitHub Actions service      dd-build-server
       |
       | runs-on: oresoftware-ci
       v
+----------------------+    +----------------------+
| AWS ARC scale set    |    | Hetzner ARC scale set|
| group: ...-aws       |    | group: ...-hetzner   |
| min 0 / max 4        |    | min 0 / max 4        |
+----------------------+    +----------------------+
```

Both ARC scale sets use the same `runnerScaleSetName`, so workflows use one
stable `runs-on` label. They use distinct runner groups because GitHub requires
same-named scale sets to be separated by group. With both online, assignment is
an availability race rather than a configurable AWS/Hetzner preference.

## What the webhook fallback can and cannot do

The webhook catches completed failure-like `workflow_run` events. It is useful
for ordinary CI failures, runner startup failures, timeouts, cancellations, and
provider-side action-required outcomes when GitHub emits a completed run.

It is not a guaranteed substitute for routing jobs to self-hosted runners in
the first place. A spending-limit condition may prevent a hosted job from
starting or may not produce the same failure event in every case. Expensive or
critical jobs should be moved directly to `runs-on: oresoftware-ci`; the signed
failure bridge is a second line of defense and an audit/event integration.

## Trust boundary

The bridge rejects:

- missing or invalid HMAC-SHA256 signatures;
- malformed or oversized delivery identifiers;
- duplicate deliveries in the bounded in-memory cache;
- events other than `workflow_run` and actions other than `completed`;
- non-hex or unexpected-length commit IDs;
- fork-originated runs whose head repository differs from the event repository;
- source events other than `push` or explicitly allowed `workflow_dispatch`;
- conclusions outside the fixed failure-like set;
- repository, workflow, branch, and conclusion values not in static rules;
- recursive workflow-dispatch rules;
- arbitrary caller-supplied commands, scripts, images, URLs, or runner labels.

The first production deployment is single-replica with in-memory delivery
suppression. Downstream `dd-build-server` requests carry a deterministic request
ID. The repository fallback workflow also uses a source-run concurrency key.
For strict cross-restart dispatch exactly-once behavior, move webhook delivery
ownership into the existing Postgres/NATS lifecycle before scaling the bridge
horizontally.

Self-hosted runners execute repository code. Limit runner-group access to
trusted repositories and trusted branches. Never route public fork pull
requests to these runners.

## Bootstrap and activation

### 1. Publish and pin images

Merge `.github/workflows/gha-clone-images.yml`. It builds the bridge and runner
images on pull requests and publishes branch/SHA tags on `main`.

After the first successful publish, replace:

```text
ghcr.io/oresoftware/gha-clone-server:main
ghcr.io/oresoftware/oresoftware-ci-runner:main
```

with immutable digests. Do not enable the organization webhook or cluster
activation label while either workload still uses a mutable bootstrap tag.

### 2. Align ARC chart versions

The new fleet targets ARC chart `0.14.2`. Upgrade the active
`canonical-ci-arc-controller` and every active runner scale set together. Render
both charts, inspect CRD changes, canary one cluster, and retain the prior chart
version for immediate rollback.

### 3. Create runner groups

Create these organization runner groups and grant only the selected trusted
repositories access:

```text
oresoftware-ci-aws
oresoftware-ci-hetzner
```

### 4. Install GitHub App secrets

Create a GitHub App installed on `ORESoftware` with the minimum repository and
organization permissions required by ARC. In namespace
`arc-runners-oresoftware` on each cluster, materialize:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: oresoftware-arc-github
  namespace: arc-runners-oresoftware
stringData:
  github_app_id: "REDACTED"
  github_app_installation_id: "REDACTED"
  github_app_private_key: |
    -----BEGIN RSA PRIVATE KEY-----
    REDACTED
    -----END RSA PRIVATE KEY-----
```

Store the source values in the existing secret manager/External Secrets flow;
never commit this object.

### 5. Activate clusters one at a time

Each Argo cluster secret already needs `dd.dev/managed=true` and
`dd.dev/cloud=aws|hetzner`. Add:

```text
dd.dev/ci-runners=oresoftware
```

first to Hetzner, verify one ephemeral job, then to AWS. The ApplicationSets
will create a controller Application and a runner-scale-set Application in each
selected cluster.

### 6. Smoke the runner before the webhook

Manually dispatch `Self-hosted fallback` using an exact commit on `main`. Verify:

- one runner pod is created from zero;
- the job lands in either permitted runner group;
- the exact SHA is checked out;
- Python contracts and Rust tests pass;
- the pod is deleted after the job;
- no long-lived registration token appears in logs or pod environment dumps.

### 7. Register the organization webhook

Use a freshly issued organization-owner credential with organization webhook
write permission. Pass credentials only via the environment:

```bash
export GH_TOKEN='REDACTED-NEW-CREDENTIAL'
export GITHUB_ORG='ORESoftware'
export GITHUB_WEBHOOK_URL='https://hello.95-217-171-250.sslip.io/ci/github/webhook'
export GITHUB_WEBHOOK_SECRET='REDACTED-AT-LEAST-32-BYTES'

python3 scripts/ops/register_github_org_webhook.py --dry-run
python3 scripts/ops/register_github_org_webhook.py
unset GH_TOKEN GITHUB_WEBHOOK_SECRET
```

The registrar lists existing hooks, matches the exact URL, and either creates or
updates one active `workflow_run` hook. It never prints the request/response
body, token, or shared secret.

### 8. Redelivery test

Trigger a controlled failure in the allowlisted `repo checks` workflow on a
trusted branch. Confirm a fallback dispatch. Redeliver the same webhook and
confirm the live process reports it as a duplicate. Then temporarily make the
downstream dispatch fail and confirm GitHub redelivery is accepted after the
failed attempt is removed from the cache.

## Operating metrics

Scrape `GET /metrics` on port 8117. Alert on:

```text
gha_clone_webhooks_rejected_total
gha_clone_fallbacks_failed_total
```

Track received, ignored, duplicate, dispatched, and failed rates. A sudden rise
in rejected signatures suggests a secret mismatch or unsolicited traffic. A
rise in ignored events may indicate workflow-name drift from the static rules.

## Rollback

1. Set the GitHub organization webhook `active=false` or delete it.
2. Remove `dd.dev/ci-runners=oresoftware` from the Argo cluster secrets.
3. Let active jobs finish, then set both generated scale sets to
   `minRunners=0,maxRunners=0` if queue draining is required.
4. Revert the bridge and fleet Argo Applications.
5. Roll back ARC controller and scale-set charts as one compatible version set.
6. Preserve sanitized delivery IDs, source run IDs, job URLs, and metrics for the
   incident record; preserve no credentials.
