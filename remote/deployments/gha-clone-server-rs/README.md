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
images, environment variables, or Kubernetes manifests. It submits only a
trusted repository, immutable commit SHA, and operator-reviewed profile name to
`dd-build-server`. Messaging Intel uses a dedicated two-job mirror:
`node-hardened-verify` for the non-secret operator contract and
`node-hardened-test` for the lifecycle-script-free complete repository test
suite.

## API

All `/v1/*` endpoints require `x-gha-clone-auth` or `x-server-auth`, compared in
constant time over SHA-256 digests.

- `GET /v1/capabilities` — supported ARC lanes, independent profiles, limits,
  and explicit exclusions.
- `POST /v1/plans` — parse workflow YAML and return per-job parity/support data.
- `POST /v1/runs` — enqueue a fully supported immutable plan.
- `GET /v1/runs/<uuid>` — inspect sequential build-server submissions.
- `POST /webhooks/github` — verify `X-Hub-Signature-256` and a UUID
  `X-GitHub-Delivery`; for `workflow_run`, accept only the completed terminal
  phase, configured failure conclusions, the exact reviewed workflow path, and
  a workflow name outside the recursion-exclusion set. The service then fetches
  that path at the event's exact SHA, plans it, and either returns the plan or
  performs a bounded, deduplicated independent dispatch.
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
| npm install-script suppression + complete repository tests | `node-hardened-test` |
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
terminal result. Duplicate delivery is bounded by the deterministic request ID,
the webhook-delivery claim, and the build server's idempotency path.

## Messaging Intel mirror and adversarial proof

The allowlisted fixture
`tests/fixtures/msgint-operator-config.yml` represents only
`messaging-intel/msgint-connectors` at an immutable 40-hex revision. Its command
sequences are exact contracts:

```text
node-hardened-verify:
  npm ci --ignore-scripts
  npm run check
  npm run test:operator-config
  npm audit --audit-level=high

node-hardened-test:
  npm ci --ignore-scripts
  npm test
```

A job that signals hardened Node intent cannot fall back to generic
`node-verify`. Extra commands, reordered commands, quoted or spoofed evidence,
mutable setup-action refs, caller-selected environments, setup-input
expressions, and secret expressions are rejected before any build submission.
The real-process integration test first proves the two valid submissions reach
`succeeded`, then sends adjacent malicious variants and verifies the recording
build server remains at exactly two submissions.

The optional hosted smoke is manual-only. It mints a short-lived GitHub App token
restricted to `messaging-intel/msgint-connectors`, checks out the exact reviewed
revision with persisted Git credentials disabled, extracts only the two compiled
fixed scripts, resolves the Node runner image to a digest, and executes both
profiles in capability-dropped, no-new-privileges, read-only containers.

## Failure webhook contract

GitHub emits `workflow_run` completion events for every conclusion. The
continuity service therefore applies all of these checks before fetching or
submitting work:

1. the raw request body has a valid SHA-256 HMAC;
2. `X-GitHub-Delivery` is a valid UUID;
3. the repository is exactly allowlisted;
4. `workflow_run.head_sha` is a full 40-hex commit SHA;
5. `action` is `completed`;
6. `conclusion` is in `GHA_CLONE_WEBHOOK_FAILURE_CONCLUSIONS`;
7. `workflow_run.name` is not in `GHA_CLONE_WEBHOOK_IGNORED_WORKFLOWS`;
8. `workflow_run.path` exactly matches one configured path for that repository;
9. the fetched workflow plans successfully and every job is independently
   executable;
10. the delivery UUID has not already claimed an independent dispatch within
    the bounded retention window.

The delivery claim is inserted only after workflow retrieval, planning, and
execution-readiness checks succeed. A transient GitHub fetch or planning failure
therefore remains retryable with the same GitHub delivery ID. Concurrent copies
of the same delivery are serialized through one in-process claim and can create
at most one independent run set.

Delivery retention is intentionally bounded by both a TTL and a maximum entry
count. It is in-memory in the first deployment, so keep exactly one replica.
Horizontal scaling requires a shared durable delivery store or a Fiducia-fenced
claim before webhook execution may be enabled.

## Fail-closed exclusions

The independent lane rejects:

- branch/tag execution instead of a 40-hex commit SHA;
- every caller-selected job/step environment;
- secret/OIDC expressions in setup inputs or commands;
- expressions inside setup inputs;
- mutable setup-action references;
- hardened Node command sequences that are incomplete, reordered, spoofed, or
  contain any extra command;
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
| `GHA_CLONE_GITHUB_API_BASE_URL` | GitHub API origin; production default `https://api.github.com`, with HTTP accepted only for loopback tests |
| `GHA_CLONE_BUILD_SERVER_URL` | internal `dd-build-server` origin |
| `GHA_CLONE_BUILD_SERVER_AUTH` | scoped build-server auth |
| `GHA_CLONE_ALLOWED_REPOSITORIES` | exact comma-separated `owner/repo` allowlist |
| `GHA_CLONE_WORKFLOW_RULES_JSON` | exact map of repository to one or more `.github/workflows/*.yml` or `.yaml` paths |
| `GHA_CLONE_EXECUTION_ENABLED` | independent API execution, default `false` |
| `GHA_CLONE_WEBHOOK_EXECUTION_ENABLED` | webhook execution, default `false` |
| `GHA_CLONE_WEBHOOK_FAILURE_CONCLUSIONS` | comma-separated terminal conclusions eligible for fallback |
| `GHA_CLONE_WEBHOOK_IGNORED_WORKFLOWS` | exact workflow names excluded to prevent fallback recursion |
| `GHA_CLONE_WEBHOOK_DELIVERY_TTL_SECONDS` | nonzero in-memory delivery-deduplication TTL |
| `GHA_CLONE_MAX_WEBHOOK_DELIVERIES` | nonzero upper bound on retained delivery UUIDs |
| `GHA_CLONE_MAX_WORKFLOW_BYTES` | parser input bound |
| `GHA_CLONE_MAX_JOBS` | workflow job bound |
| `GHA_CLONE_MAX_STEPS_PER_JOB` | per-job step bound |
| `GHA_CLONE_BUILD_TIMEOUT_SECONDS` | terminal build wait bound |

Repository names and workflow rules are validated at startup. Invalid repository
syntax, paths outside `.github/workflows`, traversal, backslashes, duplicate
paths, empty rule lists, unsafe GitHub API origins, and zero retention bounds
cause the process to exit before binding its network listener.

Use a GitHub App and External Secrets. Do not put classic PATs, private keys, or
shared secrets in source, Argo parameters, Linear, logs, URLs, or image layers.

## Register the GitHub failure webhook

Run `scripts/register-github-webhook.sh` only after the HTTPS ingress and
ExternalSecret value exist. The script reads `GH_TOKEN` and
`GITHUB_WEBHOOK_SECRET` from the environment, updates an existing hook with the
same URL or creates one, sends request bodies through stdin, and never prints
either secret.

`ORESoftware` is a GitHub user account, so register a repository hook:

```console
GH_TOKEN=... GITHUB_WEBHOOK_SECRET=... \
  bash scripts/register-github-webhook.sh \
  --repo ORESoftware/k8s-cluster \
  --url https://ci.example.com/webhooks/github
```

For an actual GitHub organization, use `--org <organization>`. The registration
script subscribes only to `workflow_run`; GitHub sends every completed
conclusion and the Rust service performs the failure-only, exact-path,
recursion, and duplicate-delivery checks.

## Deployment state

The `dd-next-runtime` manifests install the service with `replicas: 0` and
execution disabled. This is intentional: merge is safe before credentials
exist. Activation requires:

1. provision `dd-gha-clone-server-secrets` through the reviewed ExternalSecret;
2. verify the GitHub App is installed only on allowlisted organizations/repos;
3. confirm `dd-build-server` is healthy and its fixed profiles include the
   reviewed monorepo Rust fallback plus both Messaging Intel hardened profiles;
4. pin the deployment to the merged source revision;
5. scale to one replica and run plan-only fixtures;
6. enable API execution for immutable trusted commits and submit
   `.github/workflows/gha-clone-server-meta.yml` at the exact merged SHA;
7. verify the real build-server job tests this crate and the run reaches
   `succeeded` without creating a second independent run;
8. run the manual private Messaging Intel smoke after the exact-repository
   GitHub App installation and secrets are available;
9. register the failure-only `workflow_run` webhook and prove HMAC, exact-path
   filtering, recursion exclusion, retry after transient retrieval failure,
   concurrent duplicate suppression, build-server idempotency, and exact
   repository admission;
10. enable webhook execution only while the deployment remains single-replica,
    until shared delivery persistence or Fiducia fencing is implemented.

AWS is the initial independent executor because the existing build server,
containerd/buildkit, ECR and Postgres are there. Hetzner can immediately host ARC
non-privileged lanes. A second independent executor requires shared artifact
storage and Fiducia-fenced claims before it becomes authoritative.

## Repository extraction

The service is initially in `ORESoftware/k8s-cluster` so implementation,
deployment, security, and executor contracts evolve atomically. Extraction to a
standalone `ORESoftware/gha-clone-server.rs` repository is tracked after the API
and fixtures stabilize and must use the protected repository-bootstrap path.
