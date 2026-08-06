# Signed GitHub webhook activation for `gha-indie-worker`

This runbook activates GitHub `push` and `pull_request` deliveries as the primary trigger for the independent workflow compiler and worker. A `workflow_run` event may be accepted as a compatibility signal, but the continuity lane must not wait for GitHub-hosted Actions to fail before independent testing begins.

Tracking: `ORESoftware/k8s-cluster#1093` and Linear `DEN-1863`.

## Current continuity modes

### Trusted-main SSM bridge

The merged DES browser lane uses a small GitHub-hosted job only to authenticate with AWS, invoke a checksum-pinned script through SSM, and collect evidence. Playwright and Puppeteer execution runs through `dd-build-server` and `gha-indie-worker` at exact commit SHAs.

This is suitable for tonight's continuity work because it moves the expensive test compute off GitHub-hosted runners. It is not the final independent trigger path.

### Direct signed webhook

The durable path is:

```text
GitHub push / pull_request
        |
        | application/json + X-Hub-Signature-256 + X-GitHub-Delivery
        v
dd-remote-gateway
        |
        v
dd-gha-clone-server:8125/webhooks/github
        |
        | exact repository + exact workflow path + full immutable SHA
        | fail-closed workflow planner -> fixed reviewed profile
        v
dd-build-server -> gha-indie-worker
        |
        v
GitHub status/check context: gha-indie/<profile>
```

The webhook and worker must never accept caller-selected commands, images, executors, deployment targets, credentials, mutable action references, arbitrary repository prefixes, or arbitrary workflow paths.

## Phase 0: read-only static preflight

Run on the protected cluster host or another environment with read-only access to the intended cluster:

```bash
scripts/ops/preflight_gha_clone_webhook.sh
```

The preflight verifies without decoding or printing secrets:

- `ExternalSecret/dd-gha-clone-server-secrets` reports `Ready=True`;
- the target Secret contains non-empty `auth_secret`, `github_webhook_secret`, `github_app_installation_token`, and `build_server_auth` entries;
- repository and workflow-path rules are non-empty, exact, and internally consistent;
- both execution flags remain `false`;
- the pod is non-root, does not mount a service-account token, drops Linux capabilities, and listens on port 8125;
- the Service and NetworkPolicy preserve the gateway/build-server/HTTPS boundary.

A failure at this phase blocks scaling, routing, and webhook installation.

## Phase 1: internal plan-only replica

After the static preflight passes, review a GitOps change that sets only:

```yaml
spec:
  replicas: 1
```

Keep both values unchanged:

```yaml
GHA_CLONE_EXECUTION_ENABLED: "false"
GHA_CLONE_WEBHOOK_EXECUTION_ENABLED: "false"
```

After Argo CD reconciliation and rollout health, run:

```bash
scripts/ops/preflight_gha_clone_webhook.sh --probe-live
```

The live probe opens a local port-forward and verifies `/healthz` and `/readyz`. It does not send a webhook, decode a secret, or mutate the cluster.

## Phase 2: dedicated gateway route

Add a dedicated external route such as:

```text
/gha-webhooks/github -> dd-gha-clone-server.default.svc.cluster.local:8125/webhooks/github
```

Do not overload the existing `dd-build-server` webhook route. Preserve the raw request body and the following headers:

- `Content-Type: application/json`
- `X-Hub-Signature-256`
- `X-GitHub-Event`
- `X-GitHub-Delivery`

The route must not require an operator browser cookie, but the clone server must reject missing or invalid HMAC signatures. Apply request-size and rate limits at the gateway while leaving the application planner's tighter workflow limits in force.

AWS currently serves through the gateway's own hostPort/TLS path; the ingress-nginx object is inert there. Prove the exact cloud-specific route rather than assuming an Ingress change covers every cluster.

## Phase 3: GitHub webhook installation

Configure the GitHub App, organization webhook, or exact repository webhook with:

- content type `application/json`;
- the secret mapped to `github_webhook_secret`;
- TLS verification enabled;
- `push` and `pull_request` events;
- optional `workflow_run` only as a compatibility/fallback signal.

Start with one exact `*-test` repository and one or two exact workflow paths. Do not allow an entire organization prefix merely because the repositories share an owner.

Before enabling execution, prove and retain redacted evidence for:

1. `ping` delivery;
2. invalid-signature rejection;
3. missing or oversized delivery-ID rejection;
4. duplicate-delivery idempotency;
5. non-allowlisted repository rejection;
6. malformed and non-full SHA rejection;
7. unapproved workflow-path rejection;
8. unsupported workflow semantics failing closed;
9. plan-only success at the event's exact commit SHA.

## Phase 4: staged execution

Enable the two gates separately.

First enable authenticated manual execution:

```yaml
GHA_CLONE_EXECUTION_ENABLED: "true"
GHA_CLONE_WEBHOOK_EXECUTION_ENABLED: "false"
```

Prove one exact-SHA run through the authenticated `/v1/runs` endpoint. Verify the build-server request contains only `run-profile`, the exact repository URL, the exact commit SHA, the fixed reviewed profile, and an idempotent request ID.

Then enable webhook execution for the exact pilot:

```yaml
GHA_CLONE_EXECUTION_ENABLED: "true"
GHA_CLONE_WEBHOOK_EXECUTION_ENABLED: "true"
```

Prove success, test failure, replay, transient GitHub API failure, transient build-server failure, timeout, and restart recovery before expanding the allowlist.

## Phase 5: GitHub-visible status and merge policy

Independent execution does not automatically satisfy an existing required GitHub Actions check name. Publish a distinct least-privilege context, for example:

```text
gha-indie/playwright
gha-indie/puppeteer
gha-indie/rust-verify
```

The lifecycle must include pending, success, failure, timeout, cancellation, superseded, and lost-worker recovery. Status publication failures require bounded retry and dead-letter handling keyed by repository, SHA, workflow path, plan ID, and delivery ID.

Only after parity evidence is complete should branch protection or the overnight merge policy trust the new context. Never impersonate an unrelated GitHub Actions check name and never bypass required checks.

## Overnight introspection contract

For repositories migrated to the direct webhook lane, an overnight agent should:

1. create or reuse an idempotent branch and PR;
2. record the exact pushed head SHA;
3. verify GitHub delivered the signed event;
4. verify the clone server accepted the exact repository/workflow/SHA tuple exactly once;
5. observe terminal `gha-indie/<profile>` evidence for that same SHA;
6. merge only when repository policy permits and every required independent context is successful;
7. retain repository, branch, PR, SHA, delivery ID, plan ID, run ID, build IDs, status contexts, and evidence links in the recovery ledger;
8. classify missing webhook, missing status, unsupported workflow, absent runner capacity, or failed publication as explicit unfinished states rather than silently treating them as success.

Repositories that have not migrated remain on the trusted-main SSM bridge or their existing CI. The agent must not assume the independent lane covers a repository merely because another repository in the same organization is allowlisted.

## Rollback

Rollback is fail-closed and must not rotate or expose secrets merely to stop execution:

1. set `GHA_CLONE_WEBHOOK_EXECUTION_ENABLED=false`;
2. if necessary set `GHA_CLONE_EXECUTION_ENABLED=false`;
3. remove or disable the external webhook route;
4. scale `dd-gha-clone-server` to zero;
5. preserve delivery, plan, run, and status-publication evidence for diagnosis.

Do not delete the webhook secret during an incident unless compromise is suspected; rotation and delivery shutdown are separate operations.
