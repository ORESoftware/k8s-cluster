# Signed GitHub Actions failure fallback for `gha-indie-worker`

Tracking: `ORESoftware/k8s-cluster#1093`, Linear `DEN-1863`, and Linear `DEN-1597`.

## What this activation does

This GitOps state runs one digest-pinned `dd-gha-clone-server` replica and one
digest-pinned `dd-gha-executor-router` replica. It exposes one exact HTTPS path:

```text
https://hello.95-217-171-250.sslip.io/gha-webhooks/github
```

The GitHub hook subscribes only to `workflow_run`. The pinned clone server accepts
only a completed run whose conclusion is in the configured failure set, whose
repository is exactly allowlisted, whose workflow path is exactly configured,
and whose `head_sha` is a full immutable commit ID. It then fetches the workflow
YAML at that SHA, compiles only the supported bounded subset, and submits fixed
reviewed profiles through the executor router.

The active pilot is intentionally limited to:

```text
ORESoftware/k8s-cluster
  .github/workflows/gha-continuity-parity.yml
  .github/workflows/remote-k8s-browser-suite.yml
```

The router may send accepted work only to the enabled AWS `dd-build-server`
profile endpoint. The Hetzner executor identity remains disabled and has no URL
or credential path. The clone server cannot choose a provider or submit caller-
selected commands, images, workflow paths, repository prefixes, or credentials.

## Important budget boundary

GitHub does not emit a distinct repository webhook whose meaning is “the Actions
budget is empty.” The immediate continuity signal is therefore a signed failed
`workflow_run`, including startup/allocation failures. It can also mirror an
ordinary failed run because the current pinned server does not independently
query organization billing.

Exact budget-aware routing requires the separately scoped
`gha-capacity-broker-rs` to read organization Actions usage through a dedicated
billing GitHub App and authorize the event before compilation. That broker is
not activated by this change: its current Kubernetes examples are digest-gated
templates, and no reviewed production image or billing-App secret is admitted.
Do not describe this pilot as exact billing detection or full GitHub Actions
compatibility.

## Signed delivery contract

The public ingress preserves the raw request body and forwards these headers:

- `Content-Type: application/json`
- `X-Hub-Signature-256`
- `X-GitHub-Event`
- `X-GitHub-Delivery`

The clone server then enforces all of the following before dispatch:

1. valid HMAC-SHA256 over the original body;
2. a UUID delivery ID and bounded replay retention;
3. exact repository allowlisting;
4. `workflow_run.action == completed`;
5. an allowed failure conclusion;
6. a full 40-character lowercase commit SHA;
7. an exact configured workflow path;
8. successful workflow fetch at that immutable SHA;
9. successful fail-closed compilation into fixed independent profiles;
10. deduplication before run reservation and submission.

GitHub's initial signed `ping` is acknowledged with HTTP 202 but never dispatched,
because only `workflow_run` is executable.

## Read-only activation verification

Run from a host with read-only access to the intended cluster:

```bash
scripts/ops/verify_gha_workflow_run_fallback.sh
```

To include the public-route check:

```bash
EXTERNAL_WEBHOOK_URL='https://hello.95-217-171-250.sslip.io/gha-webhooks/github' \
  scripts/ops/verify_gha_workflow_run_fallback.sh
```

The verifier never prints or decodes secret values. It checks:

- both ExternalSecrets report `Ready=True`;
- all required Secret keys exist and contain non-empty encoded data;
- both Deployments request and expose one available replica;
- clone API execution, clone webhook execution, and router execution are `true`;
- both images remain at the reviewed immutable digests;
- `/healthz` and `/readyz` succeed through local port-forwards;
- the optional public endpoint reaches the application and rejects an unsigned
  delivery with HTTP 401 rather than returning an edge or upstream error.

A missing Secret, unavailable replica, failed readiness endpoint, 404, 502, 503,
or TLS failure means the service is not live. Do not install or leave the GitHub
hook active until those checks pass in the target cluster.

The Ingress is claimed only in clusters with the `nginx` IngressClass and the
`gateway-public-tls` certificate. The AWS hostPort gateway does not claim this
Ingress and still needs its own exact route before GitHub can reach an AWS clone
server directly.

## Register the exact pilot webhook

Use a short-lived hook-administration credential or a least-privilege GitHub App
installation token. The HMAC value must already match the cluster's
`github_webhook_secret` property.

```bash
export GH_TOKEN='...short-lived hook-admin credential...'
export GITHUB_WEBHOOK_SECRET='...same secret already held by External Secrets...'

remote/deployments/gha-clone-server-rs/scripts/register-github-webhook.sh \
  --repo ORESoftware/k8s-cluster \
  --url https://hello.95-217-171-250.sslip.io/gha-webhooks/github

unset GH_TOKEN GITHUB_WEBHOOK_SECRET
```

The script upserts one active repository hook, enables TLS verification, uses
JSON delivery, and subscribes only to `workflow_run`. It sends the payload to
`gh api` over stdin and never echoes either credential.

Retain redacted evidence for:

1. the signed `ping` delivery returning 202;
2. an invalid-signature delivery returning 401;
3. a missing or malformed delivery ID returning 400;
4. a non-allowlisted repository failing closed;
5. a non-full SHA returning 422;
6. an unapproved workflow path being ignored;
7. a successful exact-SHA fetch and plan;
8. duplicate delivery producing no second dispatch;
9. one terminal fixed-profile build with repository, SHA, workflow path,
   delivery ID, clone run ID, router request ID, and build ID preserved.

## GitHub-visible status

The current pinned clone server retains run state internally but does not yet
publish a complete pending/success/failure Check Run lifecycle. Keep branch
protection unchanged and do not impersonate an existing Actions check. Distinct
`gha-indie/<profile>` publication, retry, recovery, and dead-letter behavior
remain acceptance work in `DEN-1863`.

## Rollback

Rollback is fail-closed and does not require deleting secrets:

1. disable or remove the repository webhook;
2. set `GHA_CLONE_WEBHOOK_EXECUTION_ENABLED=false`;
3. set `GHA_CLONE_EXECUTION_ENABLED=false`;
4. set `GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED=false`;
5. scale both Deployments to `replicas: 0`;
6. remove the exact Ingress route if external intake must stop immediately;
7. preserve delivery, run, router, build, log, and artifact evidence.

Rotate the webhook secret only when compromise is suspected. Delivery shutdown
and secret rotation are separate incident actions.
