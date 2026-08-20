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
  -> ingress-nginx exact path /gha-webhooks/github
  -> dd-gha-clone-server:8125/webhooks/github
  -> exact repo + SHA + workflow YAML planner
  -> dd-gha-executor-router:8126
  -> dd-build-server:8100 fixed profile
  -> dd-ci-profile-runner:8147 isolated host-containerd execution
```

The AWS host-port gateway and clusters with ingress-nginx expose only the exact webhook path. Health, readiness, run state, planner APIs, and manual-run APIs remain ClusterIP-only. The clone server has no direct NetworkPolicy path to the build server.

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
  --secret-file /secure/path/github_webhook_secret
```

The command is idempotent by exact callback URL and configures only `workflow_run`, JSON payloads, TLS verification, and an active hook. Repeat it only for repositories already present in `dd-gha-clone-server.configmap.yaml` with exact workflow-path rules.

## Live proof and exact-SHA execution canary

First prove the GitOps objects and secrets are ready without printing secret values:

```bash
kubectl -n default rollout status deployment/dd-gha-executor-router --timeout=5m
kubectl -n default rollout status deployment/dd-gha-clone-server --timeout=5m
kubectl -n default get externalsecret \
  dd-gha-clone-server-secrets dd-gha-executor-router-secrets
```

Keep private status APIs on a local port-forward. Copy secret bytes into a mode-`0600` temporary directory without echoing them:

```bash
set -euo pipefail
umask 077
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"; kill "${pf_pid:-}" 2>/dev/null || true' EXIT

kubectl -n default get secret dd-gha-clone-server-secrets \
  -o jsonpath='{.data.github_webhook_secret}' | base64 -d >"$tmp/webhook"
kubectl -n default get secret dd-gha-clone-server-secrets \
  -o jsonpath='{.data.auth_secret}' | base64 -d >"$tmp/clone-auth"

kubectl -n default port-forward service/dd-gha-clone-server 18125:8125 \
  >"$tmp/port-forward.log" 2>&1 &
pf_pid=$!

python3 scripts/ops/canary_gha_clone_budget_webhook.py \
  --repository ORESoftware/k8s-cluster \
  --sha "$(git rev-parse HEAD)" \
  --workflow-path .github/workflows/gha-clone-server-meta.yml \
  --workflow-name 'GHA continuity server meta' \
  --webhook-secret-file "$tmp/webhook" \
  --clone-auth-secret-file "$tmp/clone-auth"
```

A passing canary proves all of these in one chain: public TLS ingress, HMAC verification, repository extraction, exact commit extraction, exact-SHA workflow fetch, workflow-path allowlist, YAML planning, router authentication/readiness, build-server profile acceptance, terminal execution, and run-state polling. Its output is redacted JSON containing only delivery ID, repository, immutable SHA, workflow path, run IDs, and terminal states.

Before sending the valid canary, an unsigned POST to the public callback should return `401`; a valid but replayed delivery should be accepted as a no-op and must not create a second run.

## Rollback

One GitOps rollback disables the lane without rotating secrets:

- set `GHA_CLONE_WEBHOOK_EXECUTION_ENABLED=false`;
- set `GHA_CLONE_EXECUTION_ENABLED=false`;
- set `GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED=false`;
- scale clone server and router to `0`;
- remove the `dd-gha-clone-webhook` Ingress document or deactivate the repository hooks.

Do not change the fixed image digests, repository/workflow rules, or NetworkPolicy during an emergency rollback.
