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
commit SHA, and operator-reviewed profile name to `dd-build-server`. Messaging
Intel uses a dedicated two-job mirror: `node-hardened-verify` for the non-secret
operator contract and `node-verify` for the complete repository test suite.

## API

All `/v1/*` endpoints require `x-gha-clone-auth` or `x-server-auth`, compared in
constant time over SHA-256 digests.

- `GET /v1/capabilities` — supported ARC lanes, independent profiles, limits,
  and explicit exclusions.
- `POST /v1/plans` — parse workflow YAML and return per-job parity/support data.
- `POST /v1/runs` — enqueue a fully supported immutable plan.
- `GET /v1/runs/<uuid>` — inspect sequential build-server submissions.
- `POST /webhooks/github` — verify `X-Hub-Signature-256`, fetch allowlisted
  workflow files at the event's exact SHA, and plan or execute them.
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
| This repository's bounded GHA-clone meta workflow | `rust-verify`, with an exact reviewed fallback to `remote/deployments/gha-clone-server-rs` when the repository root is not a Cargo crate |
| npm/pnpm/yarn/Node tests | `node-verify` |
| npm install-script suppression + operator checks + high-severity audit | `node-hardened-verify` |
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

## Meta self-test: the server uses itself

`.github/workflows/gha-clone-server-meta.yml` is deliberately limited to the
independent compiler's supported subset. It describes one Rust verification job
for this service and contains no secrets, dynamic expressions, matrices,
conditions, service containers, caller-selected working directory, or mutable
revision.

`tests/meta_self_test.rs` starts the real `gha-clone-server` binary, starts a
recording build-server double, submits that exact workflow through the running
server's authenticated `POST /v1/runs` endpoint, polls `GET /v1/runs/<uuid>` to a
terminal state, and verifies the outgoing request contains only:

- `ORESoftware/k8s-cluster`;
- an exact 40-hex commit SHA;
- `jobKind=run-profile`;
- the fixed `rust-verify` profile;
- the deterministic plan/job request ID.

That gives CI an end-to-end test of the real HTTP server, authentication,
planner, run store, topological dispatcher, build submission, polling, and
terminal-state update. The build-server double does not execute repository code;
its purpose is to keep pull-request CI hermetic and credential-free.

After deployment activation, submitting the same fixture at the exact merged SHA
uses the real `dd-build-server`. Its `rust-verify` profile first accepts a root
`Cargo.toml`; for this monorepo it has one additional exact, operator-reviewed
fallback to `remote/deployments/gha-clone-server-rs/Cargo.toml`. It performs no
filesystem search and accepts no caller-selected directory or command.

This is dogfooding, not unbounded recursion. The independent execution does not
start another GitHub Actions workflow or emit a webhook that re-submits itself;
it creates one fixed build-server job for the immutable commit and stops at its
terminal result. Duplicate delivery is bounded by the deterministic request ID
and the build server's idempotency path.

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
| `GHA_CLONE_GITHUB_TOKEN_FILE` | preferred projected file containing a rotating GitHub App installation token |
| `GHA_CLONE_GITHUB_TOKEN` | legacy inline token for local tests; mutually exclusive with the file source |
| `GHA_CLONE_BUILD_SERVER_URL` | internal `dd-build-server` origin |
| `GHA_CLONE_BUILD_SERVER_AUTH` | scoped build-server auth |
| `GHA_CLONE_ALLOWED_REPOSITORIES` | exact comma-separated `owner/repo` allowlist |
| `GHA_CLONE_WORKFLOW_RULES_JSON` | map of repository to workflow paths |
| `GHA_CLONE_EXECUTION_ENABLED` | independent API execution, default `false` |
| `GHA_CLONE_WEBHOOK_EXECUTION_ENABLED` | webhook execution, default `false` |
| `GHA_CLONE_MAX_WORKFLOW_BYTES` | parser input bound |
| `GHA_CLONE_MAX_JOBS` | workflow job bound |
| `GHA_CLONE_MAX_STEPS_PER_JOB` | per-job step bound |
| `GHA_CLONE_BUILD_TIMEOUT_SECONDS` | terminal build wait bound |

Use a GitHub App and External Secrets. Do not put classic PATs, private keys, or
shared secrets in source, Argo parameters, Linear, logs, URLs, or image layers.
The Kubernetes deployment projects the installation token as a Secret volume;
the server reads that file for every workflow fetch so broker rotation is live
without a restart. Inline and file token sources are mutually exclusive.

## Deployment state

The `dd-next-runtime` manifests install the service with `replicas: 0` and
execution disabled. This is intentional: merge is safe before credentials
exist. Activation requires:

1. provision `dd-gha-clone-server-secrets` through the reviewed ExternalSecret;
2. verify the GitHub App is installed only on allowlisted organizations/repos;
3. confirm `dd-build-server` is healthy and its fixed profiles include the
   reviewed monorepo Rust fallback;
4. pin the deployment to the merged source revision;
5. scale to one replica and run plan-only fixtures;
6. enable API execution for immutable trusted commits and submit
   `.github/workflows/gha-clone-server-meta.yml` at the exact merged SHA;
7. verify the real build-server job tests this crate and the run reaches
   `succeeded` without creating a second independent run;
8. enable webhook execution only after HMAC, duplicate-delivery, and
   idempotency evidence.

AWS is the initial independent executor because the existing build server,
containerd/buildkit, ECR and Postgres are there. Hetzner can immediately host ARC
non-privileged lanes. A second independent executor requires shared artifact
storage and Fiducia-fenced claims before it becomes authoritative.

## Repository extraction

The service is initially in `ORESoftware/k8s-cluster` so parser, executor
profiles, deployment and policy contracts evolve atomically. Extraction to a
standalone `ORESoftware/gha-clone-server.rs` repository is tracked after the API
and fixtures stabilize and must use the protected repository-bootstrap path.
