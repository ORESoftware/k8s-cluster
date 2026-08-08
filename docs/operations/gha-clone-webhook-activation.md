# Signed GitHub webhook activation for `gha-indie-worker`

This runbook activates GitHub `push` and `pull_request` deliveries as the primary trigger for the independent workflow compiler and worker. A `workflow_run` event may remain a compatibility signal, but the continuity lane must not wait for GitHub-hosted Actions to fail before independent testing begins.

Tracking: `ORESoftware/k8s-cluster#1093`, Linear `DEN-1863`, and router activation issue `DEN-1597`.

## Current continuity modes

### Trusted-main SSM bridge

The DES browser lane uses a small GitHub-hosted job only to authenticate with AWS, invoke a checksum-pinned script through SSM, and retain evidence. Playwright and Puppeteer execute through `dd-build-server` and `gha-indie-worker` at exact commit SHAs.

This moves expensive test compute off GitHub-hosted runners. It is not the final independent trigger path.

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
dd-gha-executor-router:8126
        |
        | pre-submit readiness selection; no post-attempt failover
        v
dd-build-server:8100 -> gha-indie-worker
        |
        v
GitHub status/check context: gha-indie/<profile>
```

The webhook, planner, and router must never accept caller-selected commands, images, executors, deployment targets, credentials, mutable action references, arbitrary repository prefixes, or arbitrary workflow paths.

## Authority split

The clone server and router intentionally do not share one broad Secret:

- `dd-gha-clone-server-secrets` owns `auth_secret`, `github_webhook_secret`, and the short-lived `github_app_installation_token`;
- `dd-gha-executor-router-secrets` owns only `inbound_auth` for clone-to-router requests;
- the router projects the existing `dd-agent-secrets.SERVER_AUTH_SECRET` read-only for AWS build-server authentication;
- Hetzner is disabled and has no URL or credential path.

Never place a classic PAT in either continuity Secret.

## Phase 0: read-only inert preflight

Run on the protected cluster host or another environment with read-only access to the intended cluster:

```bash
scripts/ops/preflight_gha_clone_webhook.sh
```

The preflight verifies without decoding or printing secret values:

- clone and router ExternalSecrets report `Ready=True`;
- the clone Secret has exactly the three required authorities above;
- the router Secret has non-empty `inbound_auth`;
- the existing build-server Secret has `SERVER_AUTH_SECRET`;
- clone repository and workflow-path rules are non-empty, exact, and internally consistent;
- router placement is exactly AWS enabled plus credential-free Hetzner disabled;
- clone-to-router and router-to-build-server bindings use the reviewed split authority;
- the signed, scanned clone and router images are pinned by digest;
- clone API execution, clone webhook execution, and router execution remain `false`;
- both Deployments remain at `replicas: 0`;
- pods are tokenless, non-root, read-only, and capability-dropped;
- Services expose only ports 8125 and 8126;
- NetworkPolicies enforce gateway -> clone -> router -> AWS build server, with no direct clone-to-build-server path and no public router/Hetzner egress.

A failure blocks scaling, routing, and webhook installation.

## Phase 1: internal plan-only replicas

After the inert preflight passes, review a GitOps change that sets only:

```yaml
# dd-gha-clone-server
spec:
  replicas: 1

# dd-gha-executor-router
spec:
  replicas: 1
```

Keep all execution values unchanged:

```yaml
GHA_CLONE_EXECUTION_ENABLED: "false"
GHA_CLONE_WEBHOOK_EXECUTION_ENABLED: "false"
GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED: "false"
```

After Argo CD reconciliation and both rollouts become available, run:

```bash
scripts/ops/preflight_gha_clone_webhook.sh --probe-live
```

The live probe opens local port-forwards and verifies clone and router `/healthz` and `/readyz` responses. It sends no webhook, submits no build, decodes no Secret, and performs no Kubernetes write.

## Phase 2: dedicated gateway route

Add a dedicated external route such as:

```text
/gha-webhooks/github -> dd-gha-clone-server.default.svc.cluster.local:8125/webhooks/github
```

Do not overload the existing build-server webhook route. Preserve the raw request body and these headers:

- `Content-Type: application/json`
- `X-Hub-Signature-256`
- `X-GitHub-Event`
- `X-GitHub-Delivery`

The route must not require an operator browser cookie, but the clone server must reject missing or invalid HMAC signatures. Apply request-size and rate limits at the gateway while leaving the planner's tighter workflow limits in force.

AWS currently serves through the gateway's own hostPort/TLS path; the ingress-nginx object is inert there. Prove the exact cloud-specific route instead of assuming an Ingress change covers every cluster. Cloudflare routing is a separate reviewed step after origin/TLS health is proven.

## Phase 3: signed webhook pilot

Configure the GitHub App, organization webhook, or exact repository webhook with:

- content type `application/json`;
- the secret mapped to `github_webhook_secret`;
- TLS verification enabled;
- `push` and `pull_request` events;
- optional `workflow_run` only as a fallback signal.

Start with one exact `*-test` repository and one or two exact workflow paths. Do not allow an entire organization prefix merely because repositories share an owner.

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

Enable the gates separately.

First permit router submission while clone execution stays off:

```yaml
GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED: "true"
GHA_CLONE_EXECUTION_ENABLED: "false"
GHA_CLONE_WEBHOOK_EXECUTION_ENABLED: "false"
```

Prove router readiness selects only the reviewed AWS executor and that every failure after a `POST /builds` attempt remains pinned and never falls through to another provider.

Then enable authenticated manual clone execution:

```yaml
GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED: "true"
GHA_CLONE_EXECUTION_ENABLED: "true"
GHA_CLONE_WEBHOOK_EXECUTION_ENABLED: "false"
```

Prove one exact-SHA run through authenticated `/v1/runs`. Verify the router request contains only `run-profile`, the canonical repository URL, exact commit SHA, fixed reviewed profile, and deterministic request ID.

Finally enable webhook execution for the exact pilot:

```yaml
GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED: "true"
GHA_CLONE_EXECUTION_ENABLED: "true"
GHA_CLONE_WEBHOOK_EXECUTION_ENABLED: "true"
```

Prove success, test failure, replay, transient GitHub API failure, transient executor failure, timeout, and restart recovery before expanding the allowlist.

## Phase 5: GitHub-visible status and merge policy

Independent execution does not automatically satisfy an existing required GitHub Actions check name. Publish a distinct least-privilege context, for example:

```text
gha-indie/playwright
gha-indie/puppeteer
gha-indie/rust-verify
```

The lifecycle must include pending, running, success, failure, timeout, cancellation, superseded, and lost-worker recovery. Status-publication failures require bounded retry and dead-letter handling keyed by repository, SHA, workflow path, plan ID, delivery ID, router assignment, and build ID.

Only after parity evidence is complete should branch protection or the overnight merge policy trust the new context. Never impersonate an unrelated GitHub Actions check name and never bypass required checks.

## Overnight introspection contract

For a repository migrated to the direct webhook lane, an overnight agent must:

1. create or reuse an idempotent branch and PR;
2. record the exact pushed head SHA;
3. verify GitHub delivered the signed event;
4. verify the clone server accepted the exact repository/workflow/SHA tuple once;
5. record the router assignment and provider-pinned build ID;
6. observe terminal `gha-indie/<profile>` evidence for that same SHA;
7. merge only when policy permits and every required independent context is successful;
8. retain repository, branch, PR, SHA, delivery ID, plan ID, run ID, router assignment, build IDs, status contexts, and evidence links;
9. classify missing webhook, missing status, unsupported workflow, absent capacity, or failed publication as unfinished rather than success.

Repositories not migrated remain on the trusted-main SSM bridge or their existing CI. An agent must not infer coverage merely because another repository in the same organization is allowlisted.

## Rollback

Rollback is fail-closed and does not require deleting Secrets:

1. set `GHA_CLONE_WEBHOOK_EXECUTION_ENABLED=false`;
2. set `GHA_CLONE_EXECUTION_ENABLED=false`;
3. set `GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED=false`;
4. remove or disable the external webhook route;
5. scale clone and router to zero;
6. preserve delivery, plan, assignment, run, and status-publication evidence.

Do not rotate a webhook secret merely to stop delivery unless compromise is suspected; rotation and shutdown are separate operations.
