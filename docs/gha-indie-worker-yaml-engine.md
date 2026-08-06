# GHA indie-worker YAML ingestion and execution

`remote/deployments/build-server-rs` is the canonical source for
`gha-indie-worker/gha-indie-worker.rs`. The worker accepts GitHub Actions YAML,
validates a deliberately bounded subset, compiles each supported job to an
operator-reviewed fixed profile, and submits those immutable profile jobs to the
existing sandboxed build queue.

This is not a second implementation of GitHub's proprietary Actions control
plane. GitHub-hosted runners and Actions Runner Controller remain the native
semantics lane. The indie worker is the independent continuity lane for static,
trusted Linux verification workflows.

## Authenticated API

All endpoints require one of the existing constant-time build-server auth
headers.

| Endpoint | Purpose |
| --- | --- |
| `GET /gha/workflows/capabilities` | Report parser limits, installed profiles, supported semantics, and explicit exclusions. |
| `POST /gha/workflows/plan` | Parse and validate a workflow without running it. |
| `POST /gha/workflows/runs` | Validate, preflight, enqueue, and monitor a supported immutable workflow. |
| `GET /gha/workflows/runs` | List retained workflow runs. |
| `GET /gha/workflows/runs/<runId>` | Read one workflow and its underlying build job IDs/statuses. |

Request shape:

```json
{
  "schemaVersion": "gha-indie-workflow.v1",
  "repository": "ORESoftware/example",
  "revision": "0123456789abcdef0123456789abcdef01234567",
  "workflowPath": ".github/workflows/ci.yml",
  "workflowYaml": "name: CI\non: [push]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test\n",
  "requestId": "optional-idempotency-key"
}
```

The revision may be a branch or tag for planning, but execution requires an
exact 40-hex commit SHA. The worker constructs only this downstream request:

```json
{
  "schemaVersion": "build-server.v1",
  "jobKind": "run-profile",
  "repoUrl": "https://github.com/ORESoftware/example.git",
  "gitRef": "0123456789abcdef0123456789abcdef01234567",
  "profile": "rust-verify",
  "contextDir": ".",
  "push": false,
  "executor": "local",
  "requestId": "gha:<plan-id>:<job-id>"
}
```

The YAML cannot select a shell script, image, container runtime, clone URL,
working directory, build argument, deployment, namespace, or Kubernetes object.
The build server revalidates every compiled job against its repository and
profile allowlists before the workflow is accepted.

## Supported semantics

The first release supports:

- static `jobs` mappings;
- static string or string-array `needs` dependencies;
- deterministic topological execution;
- Linux runner labels;
- immutable, known setup actions;
- run-step evidence that maps to one installed fixed profile;
- dependency failure propagation through skipped downstream jobs;
- bounded in-memory run status, request deduplication, and retention.

Recognized setup actions must be pinned to a full commit SHA:

- `actions/checkout`;
- `actions/setup-node`;
- `actions/setup-python`;
- `actions/setup-java`;
- `dtolnay/rust-toolchain`;
- `pnpm/action-setup`;
- `subosito/flutter-action`.

The planner maps unambiguous evidence to the existing profiles for Rust, Node,
Python, Flutter, Playwright, and Puppeteer. A job that mixes language toolchains
is rejected instead of being guessed into one profile.

## Fail-closed exclusions

The indie worker rejects:

- mutable repository revisions for execution;
- mutable action refs;
- arbitrary marketplace actions;
- expressions in `runs-on`, setup inputs, or commands;
- secrets and token contexts;
- workflow, job, or step environments;
- matrices, reusable workflows, conditions, services, and job containers;
- custom shells, working directories, timeouts, and `continue-on-error`;
- macOS and Windows native jobs;
- cyclic or unknown dependencies;
- YAML tags, excessive nesting, excessive node count, oversized documents,
  oversized job/step sets, path traversal, and NUL bytes.

Unsupported YAML is returned as a plan with per-job reasons when structurally
valid. It is never silently approximated or partially executed.

## Execution and retention

Execution is disabled by default:

```text
BUILD_SERVER_GHA_WORKFLOW_EXECUTION_ENABLED=false
```

Planning remains available to authenticated operators. Before enabling
execution, certify the repository/profile allowlists and the node's fixed-profile
container capacity. The deployment installs conservative bounds for bytes, YAML
nodes/depth, jobs, steps, run duration, polling, and retained runs.

Workflow jobs execute in dependency order. Each job waits for the existing build
record to reach a terminal state. A failed job causes dependent jobs to be
marked skipped; independent later roots still run. The workflow succeeds only
when every planned job succeeds.

## Standalone publication

The public `gha-indie-worker/gha-indie-worker.rs` repository is generated from
this directory by the reviewed publication workflow. The GitHub App connected to
this session can read that organization but currently cannot create a branch
there, so canonical changes must merge here first and then be republished by an
installation with Contents write access on the target organization.
