# dd-formal-methods-service

Webhook-driven analysis service that runs a pluggable pipeline of formal
methods analyzers against the head commit of every pull request it is
configured to watch.

Today the pipeline ships with seven steps. Each one self-skips when its
tool is not on `PATH` inside the runtime container, so the service stays
deployable on a stock Rust image and lets operators opt into heavier
provers only when they actually want them.

| Step          | Trigger                            | Skips when…                              |
| ------------- | ---------------------------------- | ---------------------------------------- |
| `cargo-check` | `cargo check --all-targets`        | disabled via `FORMAL_METHODS_CARGO_CHECK_ENABLED=false` |
| `cargo-test`  | `cargo test`                       | disabled via `FORMAL_METHODS_CARGO_TEST_ENABLED=false`  |
| `proptest`    | `cargo test --test <target>`       | proptest target not present              |
| `kani`        | `cargo kani`                       | `cargo-kani` not on `PATH`               |
| `verus`       | `verus --time .`                   | `verus` not on `PATH`                    |
| `dreal`       | `dreal --precision <ε> *.smt2`     | `dreal` not on `PATH`                    |
| `certora`     | `certoraRun *.conf`                | `FORMAL_METHODS_CERTORA_ENABLED=false`   |

The service is fronted by a signed-webhook endpoint so any consumer
repository can trigger it without sharing credentials beyond the HMAC
secret.

## Where it lives in the cluster

```
remote/deployments/formal-methods-service-rs/
  Cargo.toml                                                 # crate: dd-formal-methods-service, lib: formal_methods_service
  Dockerfile                                                 # optional local image (cluster uses cargo run)
  src/                                                       # axum router, analyzers, GitHub client
  tests/webhook_integration.rs
  k8s/ec2/
    dd-formal-methods-service.deployment.yaml                # hostPath repo mount + cargo run --release
    dd-formal-methods-service.service.yaml                   # ClusterIP on port 8111
    dd-formal-methods-service.ingress.yaml                   # opt-in: external HTTPS for GitHub webhook
    kustomization.yaml                                       # Argo CD source path
  templates/github-actions/
    formal-methods-trigger.yml                               # opt-in Option B trigger workflow
```

The Argo CD `Application` manifest is at
[`remote/argocd/apps/dd-formal-methods-service.application.yaml`](../../argocd/apps/dd-formal-methods-service.application.yaml).
Once merged onto `dev`, Argo CD syncs the manifests under
`remote/deployments/formal-methods-service-rs/k8s/ec2`.

Runtime model matches `dd-contract-service`: the pod uses the upstream
`rust:1.90-bookworm` image, mounts the host's checkout of the cluster
repo at `/opt/dd-next-1`, and runs `cargo run --release` from the
service directory. There is no image to push to ECR.

The container also mounts:

- `/var/cache/dd-formal-methods-service` — persistent cargo target dir,
  so a restart does not rebuild from scratch.
- `/var/lib/dd-formal-methods-service` — `WORKDIR_ROOT`, where each
  PR's transient git worktree is created (cleaned up on drop).

### Toolchain gotcha (PATH, RUSTC, CARGO)

The deployment's `args` block explicitly sets `PATH`, `RUSTC`, and
`CARGO` to absolute paths under
`/usr/local/rustup/toolchains/1.90.0-x86_64-unknown-linux-gnu/bin/`.
`bash -lc` sources `/etc/profile`, which resets `PATH` to a vanilla
Debian default and silently drops `/usr/local/cargo/bin` — the
directory where the `rust:1.90` image stages the rustup proxies.
Without the fix the rustup proxy resolves fine when invoked via
absolute path, but the real cargo subprocess then spawns `rustc`
through a PATH lookup and crashes with the misleading
`error: could not execute process rustc -vV (never executed)
Caused by: No such file or directory (os error 2)`. Keep the explicit
exports if you change the args block.

## Endpoints

| Method | Path              | Description                                          |
| ------ | ----------------- | ---------------------------------------------------- |
| GET    | `/health`         | Liveness probe                                       |
| GET    | `/ready`          | Readiness probe; reports analyzers, allowlist mode, path-filter prefixes, dedupe ring size |
| POST   | `/webhook/github` | GitHub webhook (HMAC-SHA256 signed). Returns 202 with `analysis_id` for accepted deliveries |

Webhook signature verification follows GitHub's
[`X-Hub-Signature-256` spec](https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries).
Request bodies are limited to 8 MiB.

The handler enforces (in order):

1. HMAC signature check — bad signatures get `401 Unauthorized`.
2. Event-type whitelist — only `pull_request` is dispatched; other
   events return `202 Accepted` with `status: ignored`.
3. Action whitelist — only `opened` / `reopened` / `synchronize` /
   `ready_for_review` dispatch; everything else is ignored.
4. Repo allowlist (`FORMAL_METHODS_ALLOWED_REPOS`) — defence-in-depth
   against a leaked webhook secret being pointed at unrelated repos.
5. Draft skip — draft PRs are skipped unless the action is
   `ready_for_review`.
6. Delivery dedupe — bounded LRU + TTL on `X-GitHub-Delivery` so
   GitHub's automatic retries do not duplicate analyses.
7. Path filter (`FORMAL_METHODS_PATH_PREFIXES`) — fetches the PR's
   changed-file list from the GitHub API and short-circuits with a
   `success` commit-status when nothing in scope was touched.
   Conservative on errors: on any I/O failure or truncated page list
   the pipeline runs.

Accepted deliveries spawn a background analysis task. The HTTP response
returns immediately; the analyzer pipeline reports its result back to
the PR head via commit statuses under the configured `STATUS_CONTEXT`.

## Triggering: two supported approaches

### Option A (recommended): native GitHub webhook

Configure GitHub to deliver `pull_request` events directly to the
service. Lowest latency, no CI minutes burned, single source of truth
for repo allowlist / path filter / dedupe.

1. Deploy the service (see "Argo CD bring-up" below).
2. In the consumer repo's **Settings → Webhooks → Add webhook**:
   - **Payload URL**: `https://<gateway>/formal-methods/webhook/github`
     (or whatever public URL terminates at the `dd-formal-methods-service`
     k8s `Service`; the gateway path is up to your ingress config).
   - **Content type**: `application/json`
   - **Secret**: the value of `GITHUB_WEBHOOK_SECRET` on the service.
   - **Events**: `Pull requests` only.
3. In **Settings → Branches → <protected branch>**, require the status
   check whose name matches the service's `STATUS_CONTEXT` value
   (default `formal-methods/analysis`).

### Option B (opt-in): GitHub Actions dispatch

Use the template at
[`templates/github-actions/formal-methods-trigger.yml`](./templates/github-actions/formal-methods-trigger.yml)
when you want the trigger declaratively in source control — path filter
reviewable per-PR, gates on labels / draft state / codeowners, etc.

Copy the file to `.github/workflows/formal-methods-trigger.yml` in the
consumer repo and set the two repo secrets the workflow reads:

- `FORMAL_METHODS_URL` — public base URL of the service.
- `FORMAL_METHODS_WEBHOOK_SECRET` — same value as `GITHUB_WEBHOOK_SECRET`
  on the service.

The workflow posts a GitHub-shaped payload to the same
`/webhook/github` endpoint with a properly signed body, so the service
has one code path regardless of which trigger is wired up. The
workflow no-ops cleanly when either secret is unset, so it is safe to
commit before the service is reachable.

## Operator setup checklist

1. **Secrets in AWS.** Add the `FORMAL_METHODS_GITHUB_WEBHOOK_SECRET`
   (required) and `FORMAL_METHODS_GITHUB_TOKEN` (recommended; scope
   `repo:status` + `pull_requests:read`) keys to the AWS Secrets
   Manager secret `dd/remote-dev/agent-secrets`. The
   `ExternalSecret` at
   [`../../argocd/secrets/external-secrets.yaml`](../../argocd/secrets/external-secrets.yaml)
   syncs them into the `dd-agent-secrets` Kubernetes Secret every
   15 min; the deployment reads them via `secretKeyRef`.
2. **Tighten the allowlists.** Edit the deployment env block at
   [`k8s/ec2/dd-formal-methods-service.deployment.yaml`](./k8s/ec2/dd-formal-methods-service.deployment.yaml)
   so `FORMAL_METHODS_ALLOWED_REPOS` is set to your `owner/repo` (or
   `owner/*`) and `FORMAL_METHODS_PATH_PREFIXES` lists the directories
   that should gate the pipeline. The defaults are `*` (allow all) and
   empty (run on every PR) for first-touch convenience; tighten before
   exposing the webhook to the open internet.
3. **Apply the Argo CD Application.** From a kubeconfig with cluster
   admin:
   ```
   kubectl apply -f remote/argocd/apps/dd-formal-methods-service.application.yaml
   ```
   Argo CD then syncs the kustomization at
   `remote/deployments/formal-methods-service-rs/k8s/ec2` and the pod comes up as
   `dd-formal-methods-service` in the `default` namespace. First cold
   build of the release binary takes ~1 min on a 2-vCPU EC2 node;
   restarts hit the warm `target/` cache and are sub-10s.
4. **Verify internally** before exposing externally:
   ```
   kubectl -n default exec deploy/dd-formal-methods-service -- \
     curl -sS http://127.0.0.1:8111/ready | jq .
   ```
   Expected: `status: ok`, all seven analyzers listed,
   `github_token_configured: true`, repo allowlist and path filter
   reflecting your env-var edits from step 2.
5. **Expose externally** for GitHub. The service ships as a ClusterIP
   only — to let GitHub reach the webhook you need an Ingress.
   - Edit [`k8s/ec2/dd-formal-methods-service.ingress.yaml`](./k8s/ec2/dd-formal-methods-service.ingress.yaml):
     replace `formal-methods.example.com` with your real host on both
     the `tls.hosts` and `rules.host` lines. If you do not use
     cert-manager, drop the `cert-manager.io/cluster-issuer`
     annotation and pre-create the TLS secret named
     `dd-formal-methods-service-tls` in the `default` namespace.
   - Uncomment the ingress entry in
     [`k8s/ec2/kustomization.yaml`](./k8s/ec2/kustomization.yaml).
   - Add a DNS A/AAAA record for your host pointing at the
     LoadBalancer/NLB fronting `ingress-nginx`. The cluster's per-thread
     ingress pattern is documented in
     [`../../k8s/readme.md`](../../k8s/readme.md#4-dns--tls); the same DNS
     hop applies.
   - `git commit && git push`; Argo CD syncs the Ingress within a
     minute.
6. **Pick Option A or B** from the section above and configure the
   consumer repo accordingly.
   - The GitHub webhook URL is `https://<host>/webhook/github`.
   - The `Secret` in the GitHub webhook form must match the value of
     `FORMAL_METHODS_GITHUB_WEBHOOK_SECRET` from step 1 byte-for-byte.
7. **Require the status check** in the consumer repo's
   `Settings → Branches → <protected branch>`. The check name is the
   `STATUS_CONTEXT` env value (default `formal-methods/analysis`).
   Without this step the commit status is informational only.

## Environment variables

See [`.env.example`](./.env.example) for the full list with defaults.
The values that matter most:

| Var                                 | Required | Notes                                                                                  |
| ----------------------------------- | -------- | -------------------------------------------------------------------------------------- |
| `GITHUB_WEBHOOK_SECRET`             | yes      | HMAC-SHA256 secret. Bad/missing signature → `401`.                                     |
| `GITHUB_TOKEN`                      | no       | Scope `repo:status` + `pull_requests:read`. Without it the service degrades to logging only. |
| `STATUS_CONTEXT`                    | no       | GitHub commit-status `context` (default `formal-methods/analysis`).                    |
| `FORMAL_METHODS_ALLOWED_REPOS`      | no       | CSV of `owner/repo` or `owner/*`. Empty or `*` = allow all (dev only).                 |
| `FORMAL_METHODS_PATH_PREFIXES`      | no       | CSV of prefixes; empty = run on every PR.                                              |
| `MAX_CONCURRENT_ANALYSES`           | no       | Defaults to 2. Bounds the in-flight analysis count via a semaphore.                    |
| `ANALYZER_TIMEOUT_SECS`             | no       | Per-step process timeout (default 900s).                                               |

## Local development

```sh
cd remote/deployments/formal-methods-service-rs
cp .env.example .env
# fill in GITHUB_WEBHOOK_SECRET at least
cargo run
# in another shell:
curl -sS http://127.0.0.1:8111/health
curl -sS http://127.0.0.1:8111/ready | jq .
```

For an end-to-end test that does not hit GitHub:

```sh
cargo test --all-targets
```

The integration tests under `tests/webhook_integration.rs` build the
real axum router and post synthetic deliveries through it, asserting on
the HTTP responses. They use generic placeholder repo names like
`acme/widgets`; the service has no knowledge of any specific consumer
repository.
