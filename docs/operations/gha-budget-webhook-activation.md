# GitHub Actions budget-exhaustion webhook continuity

This lane keeps a small, reviewed subset of workflows executable when GitHub-hosted Actions cannot allocate a job because the account has no remaining Actions capacity.

## Event contract

GitHub does not emit a repository webhook named “budget exhausted.” The deployed compatibility signal is therefore a terminal `workflow_run` delivery whose conclusion is `action_required`. The clone server accepts that signal only when all of the following are true:

- `X-Hub-Signature-256` matches the shared webhook secret;
- `X-GitHub-Delivery` is a valid, previously unseen UUID;
- `repository.full_name` is an exact configured `OWNER/REPO`;
- `workflow_run.head_sha` is a full immutable 40-hex commit SHA;
- `workflow_run.path` is an exact configured workflow path;
- the workflow YAML fetched from GitHub at that SHA passes the bounded fail-closed planner;
- every job maps to a fixed reviewed independent profile;
- the executor router selects a ready, configured provider.

Normal `failure`, `cancelled`, and `timed_out` conclusions are not mirrored by this rollout. This avoids rerunning ordinary failing tests. `action_required` can also represent a non-budget policy/approval block, so it is a compatibility signal rather than cryptographic proof of billing state. The longer-term capacity-broker relay should make the billing decision explicitly and send the same signed exact-SHA contract without relying on this conclusion.

## Runtime path

```text
GitHub workflow_run webhook
  -> exact no-retry /gha-webhooks/github edge
  -> dd-gha-clone-server:8125/webhooks/github
  -> exact repo + SHA + workflow YAML planner
  -> dd-gha-executor-router:8126
  -> dd-build-server:8100 fixed profile
  -> dd-ci-profile-runner:8147 isolated host-containerd execution
```

The AWS host-port gateway and clusters with ingress-nginx expose only the exact webhook path. Health, readiness, run state, planner APIs, and manual-run APIs remain ClusterIP-only. The clone server has no direct NetworkPolicy path to the build server.

Webhook POSTs must not be retried by an edge proxy. The dedicated ingress sets `proxy-next-upstream=off`, one upstream try, and request buffering off. The AWS host-port gateway renders the same two nginx location directives from the reviewed ConfigMap through a non-root, capability-dropped init container. The renderer requires exactly one webhook location and one insertion anchor and fails the gateway rollout closed on partial, duplicate, or structurally drifted directives. Application delivery UUID deduplication remains a second, independent boundary rather than a substitute for the edge rule.

The build server remains unprivileged. For the exact `ORESoftware/k8s-cluster` + `rust-verify` binding, its fixed-command adapter verifies the cloned remote, immutable detached SHA, runner image, security flags, and workspace mount before delegating to `dd-ci-profile-runner`. That dedicated service is the only privileged host-containerd boundary. It accepts no caller-selected image, command, shell, mount, network, or resource limits; runs from a vulnerability-scanned digest-pinned image; and has no Kubernetes service-account token or node-local source checkout.

## Secret authority

Do not use a classic or broad operator PAT as a runtime secret.

- `dd-gha-clone-server-secrets.github_webhook_secret`: HMAC authority shared only with exact repository hooks.
- `dd-gha-clone-server-secrets.github_token`: fine-grained token projected from the protected `dd/remote-dev/agent-secrets` record and used only to fetch exact workflow YAML. A repository-scoped GitHub App installation token remains the preferred future replacement.
- `dd-gha-clone-server-secrets.auth_secret`: private clone-server API authority.
- `dd-gha-executor-router-secrets.inbound_auth`: clone-to-router authority.
- `dd-agent-secrets.SERVER_AUTH_SECRET`: router-to-AWS-build-server authority, projected read-only into the router.

## Install or reconcile a repository hook

The operator token or GitHub App used for this one-time control-plane action needs repository **Administration: read/write**. The runtime service does not need that permission.

```bash
umask 077
scripts/ops/register_gha_clone_budget_webhook.sh \
  --repository ORESoftware/k8s-cluster \
  --secret-file /secure/path/github_webhook_secret \
  --url "https://${CURRENT_PUBLIC_NODE_IP}/gha-webhooks/github"
```

Resolve `CURRENT_PUBLIC_NODE_IP` from current AWS state; the helper deliberately has no hard-coded callback default because an EC2 public address can rotate. The command is idempotent only when zero or one hook uses the exact callback URL. Multiple matching hooks are an ambiguous replay risk and fail closed for operator repair. The helper normalizes one optional terminal line ending, validates the actual 32–4096 byte visible-ASCII HMAC value, reads it through private files rather than an environment variable or process argument, configures only `workflow_run`, and verifies JSON payloads, TLS verification, active state, and the returned hook identity. Repeat it only for repositories already present in `dd-gha-clone-server.configmap.yaml` with exact workflow-path rules.

## Live proof and exact-SHA execution canary

First prove the GitOps objects and secrets are ready without printing secret values:

```bash
kubectl -n default rollout status deployment/dd-gha-executor-router --timeout=5m
kubectl -n default rollout status deployment/dd-gha-clone-server --timeout=5m
kubectl -n default rollout status daemonset/dd-remote-gateway --timeout=5m
kubectl -n default get externalsecret \
  dd-gha-clone-server-secrets dd-gha-executor-router-secrets
```

For the two isolated `gha-indie-worker-test` repositories, use the protected activation workflow instead of copying credentials to the operator laptop. The one-shot workflow resolves the current EC2 public IP through short-lived GitHub OIDC credentials, invokes the node through SSM, fetches the activator at the exact workflow SHA and verifies its SHA-256 digest, then keeps the runtime HMAC, clone API authority, and protected hook-administration token only in process memory. The initial `dev` activation commit must include the explicit `[activate-gha-test-fallback]` marker; later unmarked pushes do not repeat activation. Once the workflow is present on the default branch it is also manually dispatchable.

The activator refuses to enable hooks unless the live ExternalSecrets, exact image digests, execution flags, repository/workflow rules, build-server bindings, final privileged profile-runner bindings, gateway no-retry revision, Services, and health endpoints all match the reviewed contract. It permits no Kubernetes mutation. It then creates or reconciles exactly one active `workflow_run` hook per test repository, asks GitHub to emit a fresh signed `ping`, requires the recorded delivery to return HTTP `202`, and runs both exact-head synthetic terminal canaries. If hook or canary proof fails, it deactivates any test hooks it touched so the lane fails closed.

The canary first sends an invalid HMAC and requires HTTP `401`. It then sends one signed exact-SHA `action_required` fixture, requires exactly one returned run ID, immediately replays the same body and delivery UUID, and requires a no-op response with no `runIds`. Finally it polls that exact run to a terminal state and re-verifies repository, SHA, and workflow path. Redirects, non-object JSON, oversized responses, mutable or uppercase revisions, public plain-HTTP status origins, and secret files outside the bounded one-line contract fail closed.

A passing synthetic canary proves public TLS ingress, HMAC verification, repository extraction, exact commit extraction, exact-SHA workflow fetch, workflow-path allowlisting, YAML planning, router authentication/readiness, build-server profile acceptance, final profile-runner admission, duplicate-delivery suppression, terminal execution, and run-state polling. Its output is redacted JSON containing only delivery ID, repository, immutable SHA, workflow path, run IDs, and terminal states. The separate hook-ping receipt proves that GitHub itself reached the registered endpoint with the configured secret.

The synthetic `workflow_run` application canary does **not** prove GitHub emitted that execution delivery or that an organization budget is exhausted. The activation workflow separately retains an exact active-hook inventory and a fresh GitHub-originated `ping` receipt; those prove hook installation and GitHub-to-cluster reachability at activation time, but they are not billing evidence. Retain an actual GitHub `workflow_run` delivery receipt and authoritative capacity-broker evidence separately when those claims are required.

## Rollback

One GitOps rollback disables the lane without rotating secrets:

- set `GHA_CLONE_WEBHOOK_EXECUTION_ENABLED=false`;
- set `GHA_CLONE_EXECUTION_ENABLED=false`;
- set `GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED=false`;
- scale clone server and router to `0`;
- remove the `dd-gha-clone-webhook` Ingress document or deactivate the repository hooks.

Do not change the fixed image digests, repository/workflow rules, or NetworkPolicy during an emergency rollback.
