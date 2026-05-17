# `dd-formal-methods-server`

A small Rust HTTP service that runs **formal-methods style reasoning** over a
codebase. It accepts three flavours of input:

1. A git repo URL (analyze a whole codebase at a given ref).
2. Inline source (synchronous one-shot check, no clone, no job).
3. A **GitHub `pull_request` webhook** (clone the PR head, diff against the
   base, analyze only the changed files, then post a finding summary back as a
   PR comment).

In all three modes the actual reasoning is the same: extract a tiny annotation
DSL from source comments, compile each verification unit to SMT-LIB, and
discharge the resulting verification conditions with the
[Z3 SMT solver](https://github.com/Z3Prover/z3). Findings come back as
structured JSON, with **counterexample models** when Z3 can falsify a goal.

It is the formal/deductive counterpart to the lint/style tooling we already
run on the same checkouts: where a linter says *"this line looks fishy"*,
this service says **"under the declared assumptions there exist values
v₁ … vₙ for which your `@ensures` is false — here is one"**.

---

## Two main use cases

### 1. Analyzing a codebase

Point the service at any git repo (https / ssh / `git@`), optionally pin a
branch or tag, optionally filter to a sub-tree, optionally restrict to certain
file extensions. The service shallow-clones the repo and walks every source
file looking for annotation blocks and (optionally) plain `if (...)` lines
whose path conditions can be checked against declared variables.

```bash
curl -s -X POST http://<host>/analyses \
  -H "x-server-auth: $SERVER_AUTH_SECRET" \
  -H 'content-type: application/json' \
  -d '{
    "schemaVersion": "formal-methods.v1",
    "repoUrl": "https://github.com/example/app.git",
    "gitRef": "main",
    "paths": ["src/"],
    "languages": ["rs", "ts"]
  }'
```

Then poll the returned `id`:

```bash
curl -s -H "x-server-auth: $SERVER_AUTH_SECRET" \
  http://<host>/analyses/<id>
```

You get back the job record with `status`, `filesScanned`, `z3Queries`, and a
`findings[]` array of structured findings.

For a quick one-shot check with no clone:

```bash
curl -s -X POST http://<host>/validate \
  -H "x-server-auth: $SERVER_AUTH_SECRET" \
  -H 'content-type: application/json' \
  -d '{
    "filename": "inc.rs",
    "source": "// @var x: Int\n// @requires x > 0\n// @ensures x + 1 > 0\nfn inc(x: i64) -> i64 { x + 1 }\n"
  }'
```

### 2. Analyzing a pull request (GitHub webhook)

The service can be wired up as a **GitHub webhook target**. On any
`pull_request` event whose `action` is `opened`, `synchronize`, `reopened`, or
`ready_for_review`, it will:

1. Verify the `X-Hub-Signature-256` HMAC against `GITHUB_WEBHOOK_SECRET`.
2. Extract the PR's `head.sha`, `base.sha`, and `head.repo.clone_url`.
3. `git init` + `git fetch --depth 1 <head_sha>` + `git checkout <head_sha>`.
4. Best-effort `git fetch --depth N <base_sha>` and
   `git diff --name-only <base_sha>..<head_sha>` to discover changed files.
5. Run the same SMT-checked analysis, **scoped to those changed files**
   (controlled by `FORMAL_METHODS_PR_DIFF_ONLY`, default `true`).
6. If `GITHUB_API_TOKEN` is configured, **post a markdown summary back as a
   PR comment** via `POST /repos/{owner}/{repo}/issues/{number}/comments`.

#### GitHub side: configuring the webhook

1. In the target repository or organisation, go to
   **Settings → Webhooks → Add webhook**.
2. Set **Payload URL** to:

   ```
   https://<your-host>/webhooks/github
   ```

3. Set **Content type** to `application/json`.
4. Set **Secret** to a long random string; this is the value you will set as
   `GITHUB_WEBHOOK_SECRET` on the server side.
5. Under **Which events would you like to trigger this webhook?**, select
   **Let me select individual events** and check only **Pull requests**.
6. Click **Add webhook**. GitHub will immediately send a `ping` event that the
   server responds to with `{ "ok": true, "event": "ping" }` (200), which
   confirms the signature is wired up correctly.

You also need a fine-grained personal access token (or GitHub App
installation token) with `pull_requests: write` on the target repo if you
want the server to post PR comments back. Set it as `GITHUB_API_TOKEN` on the
server side. If unset, the server still analyzes the PR and stores findings,
it just doesn't comment back.

#### What ends up on the PR

The comment looks like:

> **dd-formal-methods-server** — PR #123 (octocat/example)
>
> - head: `9c0a4f…`, base: `b2e110…`, job: `formal-…-7`
> - files scanned: 12, Z3 queries: 47
> - analysis scope: 6 changed paths (base..head diff)
>
> 🔎 2 finding(s): 1 error · 1 warning · 0 info
>
> | Severity | Kind | File | Line | Message |
> | --- | --- | --- | --- | --- |
> | 🔴 error | `PostconditionViolation` | `src/lib.rs` | 14 | `@ensures` can be violated |
> | 🟠 warning | `TautologyAlwaysTrue` | `src/route.rs` | 41 | `if (n >= 0)` is always true |
>
> <details><summary>Counterexamples</summary>
>
> - **PostconditionViolation** at `src/lib.rs:14` (goal: `y * y > 0`):
>   - `y = 0`
>
> </details>

If you don't want a PR comment, set `FORMAL_METHODS_PR_COMMENT_ENABLED=false`.

#### Looking up PR jobs programmatically

```bash
curl -s -H "x-server-auth: $SERVER_AUTH_SECRET" \
  https://<host>/pulls/octocat/example/123
```

Returns `{ owner, repo, number, latest: <JobRecord>, jobs: [<JobRecord>...] }`
in newest-first order so you can see all analyses that have ever run for that
PR (e.g. one per `synchronize`).

#### Webhook setup walkthrough (manual `curl` test)

You can simulate a real GitHub webhook with `curl` + `openssl` before
configuring GitHub:

```bash
SECRET="$GITHUB_WEBHOOK_SECRET"
PAYLOAD=/tmp/pr.json

cat > "$PAYLOAD" <<'JSON'
{
  "action": "opened",
  "pull_request": {
    "number": 1,
    "html_url": "https://github.com/octocat/Hello-World/pull/1",
    "head": {
      "sha": "<head_sha>",
      "ref": "feature/x",
      "repo": { "clone_url": "https://github.com/octocat/Hello-World.git" }
    },
    "base": { "sha": "<base_sha>", "ref": "main" }
  },
  "repository": { "full_name": "octocat/Hello-World" },
  "sender": { "login": "octocat" }
}
JSON

SIG=$(openssl dgst -sha256 -hmac "$SECRET" "$PAYLOAD" | awk '{print $NF}')

curl -s -X POST http://<host>/webhooks/github \
  -H 'X-GitHub-Event: pull_request' \
  -H "X-Hub-Signature-256: sha256=$SIG" \
  -H 'content-type: application/json' \
  --data-binary @"$PAYLOAD"
```

Note: use `--data-binary @file`, not `--data @file`, otherwise curl strips
newlines and the HMAC will not match.

---

## Reasoning modes

| Mode                          | What the solver is asked                                                       | How a finding is produced                                            |
| ----------------------------- | ------------------------------------------------------------------------------ | -------------------------------------------------------------------- |
| Deduction (Hoare-style)       | `⋀ requires ∧ ⋀ assume ∧ ¬ goal`                                               | `sat` ⇒ counterexample bug; `unsat` ⇒ goal proved; `unknown` ⇒ info. |
| Induction (loop)              | `⋀ requires ∧ ⋀ assume ∧ ¬ invariant`                                          | `sat` ⇒ invariant fails on entry; lightweight termination check too. |
| Path-condition propagation    | conjunction of all enclosing `if (...)` guards plus the current guard          | `unsat` outer ⇒ guard means unreachable branch; tautology if outer ⊨ guard. |

These find exactly the kind of bugs that property-based and unit testing tend
to *miss* because they only check finitely many inputs.

## Annotation DSL

Parsed out of single-line comments (`//`, `#`, `--`, `;;`). Because comments
are inert in every supported language, the same block works inside Rust, Go,
TypeScript, Java, Python, Elixir, Lua, Bash, Gleam, etc.

```
// @var x: Int
// @var y: Int
// @var ratio: Real
// @var flag: Bool
// @assume y >= 1
// @requires x > 0
// @ensures (x + y) > x
// @invariant i <= n
// @variant n - i
// @assert flag || x > 0
```

Recognised directives:

| Directive       | Meaning                                                                            |
| --------------- | ---------------------------------------------------------------------------------- |
| `@var n: T`     | Declare a logical variable of sort `T ∈ {Int, Real, Bool}`. Default sort is `Int`. |
| `@assume e`     | Unconditional fact added to the context.                                           |
| `@requires e`   | Precondition. Added to the context of every goal in the same block.                |
| `@ensures e`    | Postcondition goal: prove `requires ∧ assume ⊢ e`.                                 |
| `@invariant e`  | Loop invariant goal: same base-step check as `@ensures`.                           |
| `@variant e`    | Termination measure: must be non-negative under the invariant.                     |
| `@assert e`     | Inline obligation: prove `requires ∧ assume ⊢ e`.                                  |

Expressions support comparisons, boolean connectives, integer/real arithmetic,
`min(a,b)`, `max(a,b)`, `abs(x)`, parentheses, and `true`/`false` literals.

Any contiguous sequence of `@`-comments forms one **verification unit**. A
non-comment line ends the unit. The plain-source heuristic checks
(`tautologyAlwaysTrue` / `tautologyAlwaysFalse` / `deadNestedBranch`) only
fire when every identifier inside the `if (...)` guard is already declared
via `@var` somewhere in the file.

## HTTP API

| Method | Path                                       | Auth header           | Purpose                                                       |
| ------ | ------------------------------------------ | --------------------- | ------------------------------------------------------------- |
| GET    | `/`                                        | —                     | Service descriptor.                                           |
| GET    | `/healthz`                                 | —                     | Liveness + config + `z3 in PATH` probe.                       |
| GET    | `/metrics`                                 | —                     | Prometheus scrape.                                            |
| POST   | `/analyses`                                | `x-server-auth`       | Submit a job for a repo or inline source.                     |
| GET    | `/analyses`                                | `x-server-auth`       | List recent jobs.                                             |
| GET    | `/analyses/{id}`                           | `x-server-auth`       | Status + findings for one job.                                |
| GET    | `/analyses/{id}/logs`                      | `x-server-auth`       | Tail the job log file.                                        |
| POST   | `/validate`                                | `x-server-auth`       | Synchronous validate of inline source (no spawned job).       |
| POST   | `/webhooks/github`                         | `X-Hub-Signature-256` | GitHub `pull_request` events. HMAC-verified, no auth header.  |
| GET    | `/pulls/{owner}/{repo}/{number}`           | `x-server-auth`       | All analyses ever queued for a given PR; latest first.        |

A finding has this shape:

```json
{
  "kind": "postconditionViolation",
  "severity": "error",
  "file": "src/lib.rs",
  "line": 14,
  "endLine": 14,
  "message": "@ensures can be violated under the declared assumptions",
  "goal": "y * y > 0",
  "counterexample": { "y": "0" },
  "solverStatus": "sat",
  "reasoning": "deduction: search for ⋀ assumptions ∧ ¬ ensures"
}
```

## Configuration

| Env                                        | Default                                       | Meaning                                                  |
| ------------------------------------------ | --------------------------------------------- | -------------------------------------------------------- |
| `HOST` / `PORT`                            | `0.0.0.0:8110`                                | Bind address.                                            |
| `FORMAL_METHODS_WORK_ROOT`                 | `/var/lib/dd-formal-methods-server/jobs`      | Per-job working dir, holds the clone + log.              |
| `FORMAL_METHODS_GIT_BIN`                   | `git`                                         | Git binary used for shallow clone.                       |
| `FORMAL_METHODS_Z3_BIN`                    | `z3`                                          | Z3 binary; piped SMT-LIB v2.                             |
| `FORMAL_METHODS_ALLOWED_REPO_PREFIXES`     | _empty_ (allow-all)                           | CSV allowlist of repo URL prefixes.                      |
| `FORMAL_METHODS_ALLOWED_EXTENSIONS`        | `rs,go,ts,tsx,js,jsx,mjs,cjs,py,...`          | Extensions scanned by `WalkDir`.                         |
| `FORMAL_METHODS_MAX_CONCURRENT`            | `2`                                           | Job semaphore.                                           |
| `FORMAL_METHODS_JOB_TIMEOUT_SECONDS`       | `900`                                         | Per-job wall-clock budget for `git clone` / `git fetch`. |
| `FORMAL_METHODS_Z3_TIMEOUT_SECONDS`        | `5`                                           | Per-SMT-query timeout.                                   |
| `FORMAL_METHODS_MAX_LOG_BYTES`             | `4194304`                                     | Cap on per-job log file.                                 |
| `FORMAL_METHODS_MAX_FILES`                 | `5000`                                        | Cap on files scanned per job.                            |
| `FORMAL_METHODS_MAX_FILE_BYTES`            | `524288`                                      | Skip files above this size.                              |
| `FORMAL_METHODS_MAX_FINDINGS_PER_JOB`      | `5000`                                        | Stop emitting once exceeded.                             |
| `FORMAL_METHODS_MAX_INLINE_SOURCE_BYTES`   | `262144`                                      | Cap on `inlineSource` size on `/analyses` and `/validate`. |
| `SERVER_AUTH_SECRET`                       | _unset_                                       | When unset, all auth-gated endpoints return 503.         |
| `GITHUB_WEBHOOK_SECRET`                    | _unset_                                       | HMAC secret for `/webhooks/github`. When unset returns 503. |
| `GITHUB_API_TOKEN`                         | _unset_                                       | Bearer token used to POST PR comments. When unset, no comment is posted. |
| `FORMAL_METHODS_GITHUB_API_BASE`           | `https://api.github.com`                      | Override for GitHub Enterprise.                          |
| `FORMAL_METHODS_PR_DIFF_ONLY`              | `true`                                        | If true, restrict PR analysis to files changed in the diff. |
| `FORMAL_METHODS_PR_COMMENT_ENABLED`        | `true` if `GITHUB_API_TOKEN` is set           | Toggle PR comment posting independently of the token.    |
| `FORMAL_METHODS_PR_COMMENT_MAX_ROWS`       | `25`                                          | Max findings rendered in the PR comment table.           |
| `FORMAL_METHODS_PR_BASE_FETCH_DEPTH`       | `200`                                         | Depth used when fetching `base_sha` for diffing.         |

## Kubernetes layout

* `k8s/dd-formal-methods-server.deployment.yaml` — Deployment that runs the
  Rust binary from the host-mounted checkout via `cargo run --release`. The
  init shell installs the `z3` apt package on first boot, mirroring the way
  `dd-build-server` bootstraps `nerdctl`.
  `GITHUB_WEBHOOK_SECRET` and `GITHUB_API_TOKEN` are read from the
  `dd-agent-secrets` Secret (both are optional).
* `k8s/dd-formal-methods-server.service.yaml` — ClusterIP Service on port 8110.
* `k8s/kustomization.yaml` — minikube / local kustomize entry point.
* `k8s/ec2/kustomization.yaml` — EC2 overlay consumed by Argo CD via
  `remote/argocd/apps/dd-formal-methods-server.application.yaml`.

To expose `/webhooks/github` to GitHub, route it via your ingress/gateway —
e.g. add a rule for `https://<gateway>/formal-methods/webhooks/github` that
strips the `/formal-methods` prefix and proxies to
`dd-formal-methods-server.default.svc:8110`. The webhook endpoint itself is
authentication-free (HMAC handles auth) so no auth header rewriting is
needed in the gateway.

## Local build

```bash
cd remote/formal-methods-server-rs
cargo test
cargo run --release
```

You also need the `z3` binary in `PATH`:

```bash
brew install z3        # macOS
apt-get install -y z3  # Debian / Ubuntu
```

## Why not bind libz3?

Shelling out to the `z3` CLI keeps the Rust crate dependency-free of native
libraries (`z3-sys` requires `libz3-dev` and a working C++ toolchain), makes
the deployment container ~20 MB smaller, and means individual queries cannot
crash the long-lived server process — each query is an isolated child with a
strict `tokio::time::timeout` budget.

## Security notes

* The only external commands invoked are `git` (a fixed allowlist of
  subcommands) and `z3 -in -smt2 -T:5`, both with a stripped environment and
  a strict timeout.
* `repoUrl` and the webhook's `pull_request.head.repo.clone_url` are
  restricted to `https://`, `ssh://`, or `git@` and matched against
  `FORMAL_METHODS_ALLOWED_REPO_PREFIXES`.
* `paths[]` filters are validated to stay inside the cloned repository
  (no `..`, no absolute paths).
* All endpoints that touch a job require either `x-server-auth:
  $SERVER_AUTH_SECRET` (the JSON API) or a valid `X-Hub-Signature-256` HMAC
  over the body (the GitHub webhook).
* Public introspection (`/`, `/healthz`, `/metrics`) is open.
