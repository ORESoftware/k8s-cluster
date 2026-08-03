# `gha-clone-server-rs`

`gha-clone-server-rs` is the independent continuity half of the cluster's
GitHub Actions strategy. It does **not** claim to reproduce GitHub's proprietary
control plane. Full workflow semantics are preserved through Actions Runner
Controller (ARC) self-hosted runner scale sets; this service provides a
fail-closed fallback when the GitHub runner/allocation path is unavailable.

## Two continuity lanes

1. **Native parity — ARC.** Existing workflow YAML runs through GitHub's actual
   expression evaluator, job orchestration, marketplace actions, checks,
   artifacts, and runner protocol on ephemeral AWS/Hetzner Kubernetes runners.
2. **Independent mirror — this service.** A bounded static workflow subset is
   parsed, validated, classified, and compiled to fixed `dd-build-server`
   profiles. Unsupported behavior is reported explicitly and is never silently
   approximated.

The independent lane never forwards caller-selected shell, action code, runner
images, or Kubernetes manifests. It submits only a trusted repository, immutable
commit SHA, and operator-reviewed profile name to `dd-build-server`.

## API

All `/v1/*` endpoints require `x-gha-clone-auth` or `x-server-auth`, compared in
constant time over SHA-256 digests.

- `GET /v1/capabilities` — supported ARC lanes, independent profiles, limits,
  and explicit exclusions.
- `POST /v1/plans` — parse workflow YAML and return per-job parity/support data.
- `POST /v1/runs` — enqueue a fully supported immutable plan.
- `GET /v1/runs/<uuid>` — inspect sequential build-server submissions.
- `POST /webhooks/github` — verify `X-Hub-Signature-256` and a UUID
  `X-GitHub-Delivery`, accept only configured completed failure conclusions for
  `workflow_run`, match the failed workflow path exactly, reject fallback-loop
  workflow names, and deduplicate deliveries before dispatch.
- `GET /healthz`, `GET /readyz`.

Example plan:

```json
{
  "repository": "sonus-auris/sonus-auris-interfaces",
  "revision": "0123456789abcdef0123456789abcdef01234567",
  "workflowPath": ".github/workflows/ci.yml",
  "workflowYaml": "jobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: npm ci && npm test\n"
}
```

## Independent profile mapping

| Workflow evidence | Fixed build-server profile |
| --- | --- |
| Cargo/rustfmt/Clippy/tests | `rust-verify` |
| npm/pnpm/yarn/Node tests | `node-verify` |
| Python compile/pytest | `python-verify` |
| Flutter analyze/tests | `flutter-verify` |
| Flutter Android APK/App Bundle | `flutter-android-debug` |
| Flutter web build | `flutter-web-release` |
| Flutter Linux build | `flutter-linux-release` |
| Flutter Linux `main_desktop.dart` | `flutter-linux-desktop-entrypoint` |
| Playwright | `playwright` |
| Puppeteer | `puppeteer` |

Static `needs` dependencies are validated for unknown nodes and cycles. Runs
execute in deterministic topological order and poll each build-server job to a
terminal result before submitting its dependents.

## Fail-closed exclusions

The independent lane rejects:

- branch/tag execution instead of a 40-hex commit SHA;
- secret/OIDC expressions in `env`, `with`, or commands;
- dynamic matrices and conditional jobs/steps;
- arbitrary marketplace actions;
- job or service containers;
- macOS/iOS and Windows native execution;
- environments, deployments, reusable workflows, and caller-selected commands.

These jobs still receive an ARC classification such as `sonus-ci`,
`sonus-browser`, `sonus-ci-dind`, `sonus-android-kvm`, or
`github-hosted-native`.

## Configuration

| Variable | Purpose |
| --- | --- |
| `GHA_CLONE_AUTH_SECRET` | operator/API authentication |
| `GHA_CLONE_GITHUB_WEBHOOK_SECRET` | GitHub webhook HMAC |
| `GHA_CLONE_GITHUB_TOKEN` | short-lived GitHub App installation token for private workflow reads |
| `GHA_CLONE_BUILD_SERVER_URL` | internal `dd-build-server` origin |
| `GHA_CLONE_BUILD_SERVER_AUTH` | scoped build-server auth |
| `GHA_CLONE_ALLOWED_REPOSITORIES` | exact comma-separated `owner/repo` allowlist |
| `GHA_CLONE_WORKFLOW_RULES_JSON` | map of repository to workflow paths |
| `GHA_CLONE_EXECUTION_ENABLED` | independent API execution, default `false` |
| `GHA_CLONE_WEBHOOK_EXECUTION_ENABLED` | webhook execution, default `false` |
| `GHA_CLONE_WEBHOOK_FAILURE_CONCLUSIONS` | comma-separated terminal conclusions eligible for fallback |
| `GHA_CLONE_WEBHOOK_IGNORED_WORKFLOWS` | exact workflow names excluded from fallback recursion |
| `GHA_CLONE_WEBHOOK_DELIVERY_TTL_SECONDS` | in-memory GitHub delivery dedupe TTL |
| `GHA_CLONE_MAX_WEBHOOK_DELIVERIES` | bounded retained delivery IDs |
| `GHA_CLONE_MAX_WORKFLOW_BYTES` | parser input bound |
| `GHA_CLONE_MAX_JOBS` | workflow job bound |
| `GHA_CLONE_MAX_STEPS_PER_JOB` | per-job step bound |
| `GHA_CLONE_BUILD_TIMEOUT_SECONDS` | terminal build wait bound |

Use a GitHub App and External Secrets. Do not put classic PATs, private keys, or
shared secrets in source, Argo parameters, Linear, logs, URLs, or image layers.

## Deployment state

The `dd-next-runtime` manifests install the service with `replicas: 0` and
execution disabled. This is intentional: merge is safe before credentials
exist. Activation requires:

1. provision `dd-gha-clone-server-secrets` through the reviewed ExternalSecret;
2. verify the GitHub App is installed only on allowlisted organizations/repos;
3. confirm `dd-build-server` is healthy and its new fixed profiles are present;
4. pin the deployment to the merged source revision;
5. scale to one replica and run plan-only fixtures;
6. enable API execution for immutable trusted commits;
7. register the failure-only `workflow_run` webhook and prove HMAC, exact-path filtering,
   loop exclusion, delivery dedupe, and build-server idempotency before enabling execution.

AWS is the initial independent executor because the existing build server,
containerd/buildkit, ECR and Postgres are there. Hetzner can immediately host ARC
non-privileged lanes. A second independent executor requires shared artifact
storage and Fiducia-fenced claims before it becomes authoritative.

## Repository extraction

The service is initially in `ORESoftware/k8s-cluster` so parser, executor
profiles, deployment and policy contracts evolve atomically. Extraction to a
standalone `ORESoftware/gha-clone-server.rs` repository is tracked after the API
and fixtures stabilize and must use the protected repository-bootstrap path.

## Register the GitHub failure webhook

Use `scripts/register-github-webhook.sh` only after the HTTPS ingress and
ExternalSecret value exist. The script reads `GH_TOKEN` and
`GITHUB_WEBHOOK_SECRET` from the environment, updates an existing hook with the
same URL or creates one, sends request bodies through stdin, and never prints
either secret.

`ORESoftware` is a GitHub user account, so register a repository hook:

```console
GH_TOKEN=... GITHUB_WEBHOOK_SECRET=... \
  scripts/register-github-webhook.sh \
  --repo ORESoftware/k8s-cluster \
  --url https://ci.example.com/webhooks/github
```

For an actual GitHub organization, use `--org <organization>`. Configure only
`workflow_run`; GitHub sends every completed conclusion and this server performs
the failure-only, exact-workflow, and recursion checks. Keep one replica until
delivery retention moves to a shared store.
