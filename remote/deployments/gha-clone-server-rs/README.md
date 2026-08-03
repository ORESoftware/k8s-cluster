# gha-clone-server-rs

`gha-clone-server` is a small, fail-closed bridge between GitHub organization or repository webhooks and the two CI execution paths already used by `ORESoftware/k8s-cluster`:

1. **Official GitHub Actions Runner Controller (ARC)** — dispatch a predeclared `workflow_dispatch` workflow whose jobs target an ARC runner scale set.
2. **`dd-build-server`** — submit one fixed, operator-reviewed build profile to `POST /builds`.

It is intentionally **not** a second GitHub Actions YAML interpreter. ARC runs GitHub's real runner protocol and therefore preserves the semantics of actions, expressions, matrices, permissions, environments, artifacts, logs, and checks. This service only makes the failure-to-fallback decision.

## Security boundary

The service:

- verifies `X-Hub-Signature-256` with HMAC-SHA256 and constant-time comparison;
- requires a bounded `X-GitHub-Delivery` value and deduplicates deliveries;
- accepts only completed `workflow_run` events with failure-like conclusions;
- defaults to source event `push` and rejects fork-originated runs;
- matches exact repository, workflow, branch, source event, and conclusion allowlists;
- rejects rules that can react to `success`, `neutral`, or `skipped` conclusions;
- rejects recursive rules where the fallback workflow name equals the source workflow;
- never accepts a command, image, shell fragment, URL, or runner label from the webhook payload;
- removes a delivery from the cache after a downstream failure so GitHub redelivery remains useful;
- never logs the webhook secret, GitHub token, build-server secret, or downstream response body.

Self-hosted runners must only execute trusted repositories and trusted refs. Do not add `pull_request` to `sourceEvents` for public or forkable repositories.

## Configuration

| Variable | Required | Purpose |
|---|---:|---|
| `GHA_CLONE_GITHUB_WEBHOOK_SECRET` | yes | Shared secret configured on the GitHub webhook; minimum 32 bytes. |
| `GHA_CLONE_RULES` or `GHA_CLONE_RULES_PATH` | yes | JSON rule array. Default path: `/etc/gha-clone/rules.json`. |
| `GHA_CLONE_GITHUB_TOKEN` | for live workflow dispatch | Fine-grained PAT or, preferably, short-lived GitHub App installation token with Actions write permission for matched repositories. |
| `GHA_CLONE_BUILD_SERVER_URL` | no | Default: `http://dd-build-server.default.svc.cluster.local:8100`. |
| `GHA_CLONE_BUILD_SERVER_AUTH` | for live build profiles | `SERVER_AUTH_SECRET` accepted by `dd-build-server`. |
| `GHA_CLONE_DRY_RUN` | no | Validate and return a receipt without calling downstream services. Default: `false`. |
| `GHA_CLONE_BIND` | no | Listener, default `0.0.0.0:8117`. |
| `GHA_CLONE_DELIVERY_CACHE_SIZE` | no | In-memory delivery cache, clamped to 100–100,000; default 10,000. |

## Rule examples

Dispatch a repository-owned fallback workflow onto ARC:

```json
{
  "repo": "ORESoftware/k8s-cluster",
  "workflow": "repo checks",
  "branches": ["main", "dev"],
  "sourceEvents": ["push"],
  "conclusions": ["failure", "timed_out", "cancelled", "startup_failure"],
  "action": {
    "kind": "workflowDispatch",
    "workflowFile": "self-hosted-fallback.yml",
    "workflowName": "Self-hosted fallback",
    "dispatchRef": "main",
    "runner": "oresoftware-ci"
  }
}
```

Submit an existing fixed profile to `dd-build-server`:

```json
{
  "repo": "sonus-auris/sonus-auris-ui.dart",
  "workflow": "Flutter CI",
  "branches": ["main"],
  "sourceEvents": ["push"],
  "conclusions": ["failure", "timed_out"],
  "action": {
    "kind": "buildServerProfile",
    "profile": "flutter-android-debug",
    "executor": "local"
  }
}
```

## Endpoints

- `POST /webhooks/github` and `POST /ci/github/webhook`
- `GET /healthz`
- `GET /readyz`
- `GET /metrics`
- `GET /`

## Local verification

```bash
cargo test --manifest-path remote/deployments/gha-clone-server-rs/Cargo.toml
python3 -m unittest -v scripts/ops/test_gha_clone_server_contract.py
```
