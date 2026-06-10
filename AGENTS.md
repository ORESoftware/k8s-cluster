# Agent Context

This repo uses `AGENTS.md` as the durable local context entrypoint for coding agents.
Read this file first when starting work in the repo, then read the docs that match the
task instead of relying only on prompt history.

## Context Sources

- Read `docs/*.md` for cross-repo product or architecture notes.
- Read `agents/*.md` if that directory exists in a checkout. Treat it as agent-specific
  operating context, not application source.
- Read nested `AGENTS.md` files when working inside a subdirectory that defines one.
- Read `docs/agent-context-memory.md` for the remote-agent memory and autonomy contract.
- For this repo, the highest-value runtime runbooks are:
  - `remote/readme.md`
  - `remote/deployments/dev-server/readme.md`
  - `remote/deployments/gleam-mcp-server/readme.md`
  - `remote/argocd/vpn/readme.md`
  - `remote/ec2/README.md`

## Runtime Context

Agents launched by `remote/deployments/dev-server` may receive selected Postgres context rows in
the prompt. Treat those as task-specific memory, and treat this file plus the docs above
as persistent repo memory. The thread UI can seed a task with durable context blobs,
previous thread tasks, and individual breadcrumb rows from `agent_remote_dev_breadcrumbs`.
Rows unchecked during context review are omitted from the worker payload; "Start with zero context"
sets `contextMode: none` so previous tasks, breadcrumbs, and selected blobs are not injected. See
`remote/libs/interfaces/redis` for the optional Redis cache shape and
`remote/libs/pg-defs/schema/schema.sql` for the table contract.

When the cluster MCP server is configured as `dd_cluster`, use it before guessing live EC2
Kubernetes deployment state, service wiring, inventory, or observability status. The MCP surface is
read-only by default; do not add write-capable AWS or Kubernetes tools without a separate
short-lived human grant, auth, and audit design. Treat the EC2 Kubernetes manifests and live
`dd_cluster` output as the runtime source of truth.

## Observability Contract

Prefer collection at the process and platform boundaries over runtime-wide instrumentation. Do not
monkey-patch Node.js, Erlang, Rust, Java, framework internals, standard streams, module loaders,
HTTP clients, fetch, timers, or logging APIs for OpenTelemetry or any other telemetry path.

Container stdout/stderr is a first-class telemetry source. Services should emit either ordinary text
logs or the shared structured JSON envelope documented in
`docs/observability-stdio-contract.md`; Promtail/Loki collect those streams from Kubernetes CRI logs.
Use explicit runtime logging/event APIs instead of patching: Node may bridge explicit
`process.emit("info", payload)` / `process.emit("warning", payload)` producers with
`process.on(...)`; Rust should prefer `tracing`/`tracing_subscriber`; Java/Scala should prefer
Logback or Log4j appenders/config; Erlang/Gleam should use explicit `logger`/`io` calls or owned
actors. OpenTelemetry spans and metrics are also explicit-only.

For alert-worthy operational failures, prefer publishing compact, redacted events to the generated
NATS subject `dd.remote.events.critical` (`NATS_CRITICAL_EVENT_SUBJECT`) in addition to writing the
`dd.log.v1` stdout/stderr line. Keep routine lifecycle/status traffic on `dd.remote.events`.

## Command Safety

The following commands are blacklisted for agents in this repo: `git checkout`, `git reset`,
`git stash`, `rm`, and `sed`. Do not run them in local operator work or add them to automation
scripts. Leave untracked runtime files alone unless a human asks for a specific cleanup.

`git stash` is absolutely forbidden — never run `git stash`, `git stash push`, `git stash pop`,
`git stash apply`, `git stash drop`, or any other `git stash` subcommand under any circumstances,
even to temporarily set work aside or recover from a conflict. Stashing has repeatedly left
half-applied, conflicted, and lost work in this repo. To set changes aside, commit them to a branch
instead; to undo, commit a revert. If you encounter an in-progress stash conflict, resolve it
forward by pulling the stashed changes in and committing the resolution — do not re-stash.

Default branch posture for agent work is `dev`. Agents should not check out or switch to feature
branches for local operator work unless a human explicitly changes that posture for a specific task.
If the workspace is already on a non-`dev` branch, surface that state before doing branch-sensitive
work and prefer integrating feature work back into `dev` instead of continuing on the feature branch.

When a task is complete, run `git add -A`, inspect the staged changes, commit, and push to the
tracked remote branch. If the remote has new commits, pull them in, resolve any conflicts
semantically by preserving the intended behavior of both local and remote changes, commit the
resolution when needed, and re-push. Do not use this workflow to commit secrets or unrelated runtime
files.

When publishing executable or binary artifacts to the filesystem, prefer a temporary operator-owned
location such as `$HOME/.codex/tmp` instead of the repository tree. If a binary with the same name
already exists in the target folder, move the previous file to the user's Trash before placing the
new one; do not delete it with `rm` or silently overwrite it.

## Postgres Contract

RDS Postgres plus `remote/libs/pg-defs/schema/schema.sql` are the database contract. Do not generate
SQL, migrations, or table DDL from Rust code, API route handlers, Rust structs, or other application
code. If application code needs a schema change, update the Postgres contract manually in
`remote/libs/pg-defs/schema/schema.sql`, regenerate/check `remote/libs/pg-defs`, and then update the
custom code to match that contract.

Treat public REST routes as domain/code-first behavior for authorization, orchestration, joins,
aggregation, side effects, and product logic. Do not expose generic table-shaped CRUD as the public
API contract. If generic database inspection is needed for operators, keep it behind an explicitly
enabled internal route such as `/internal/db/*`, with service/operator auth, and keep it out of
public gateway paths.

Use `scripts/pg/diff/rds-vs-pg-defs.mjs` for declarative RDS-vs-pg-defs drift reports. The script
compares live RDS catalog state to `remote/libs/pg-defs/schema/schema.sql` and does not generate
`.sql` migration files. Treat its output as review context for human-owned manual migration work,
not as an executable migration artifact.

## API Docs Contract

HTTP API deployments should expose generated API docs at `/docs/api` and `/api/docs`, with
machine-readable metadata at `/api/docs.json`. Docs must be derived from route declarations or
equivalent runtime source using `remote/tools/generate-api-docs.mjs`; do not maintain manual route
inventories for API docs. Non-Rust runtimes may use runtime-specific generated artifacts or modules,
but they should still come from source scanning and be checked with `--check` in CI.

## Access Posture

Do not put raw AWS keys, model keys, GitHub tokens, or gateway secrets in Git. The
preferred operator path is:

- Local operator AWS access uses the shared credentials/config files in `~/.aws/credentials`
  and `~/.aws/config`, not AWS SSO. Prefer `AWS_PROFILE` or the default profile from those
  files, and verify access with `aws sts get-caller-identity` without printing secret values.
  For Terraform and AWS CLI work, use those local shared-credentials files directly; do not copy
  keys into this repo, synthesize temporary credential files, or fall back to another auth mechanism
  unless a human explicitly grants it.
- External Secrets reads AWS Secrets Manager and syncs Kubernetes secrets.
- Agents receive only the strict env allowlist defined in `remote/deployments/dev-server/src/agents`.
- Cluster access (operators and agents) uses the local AWS credentials in `~/.aws/credentials`
  (profile `dd-codex`, `region = us-east-1`) together with the `dd-ec2-runtime` kubeconfig context,
  which targets the API endpoint directly. Refresh the profile when needed (its
  `credential_process` exports short-lived credentials) and verify with
  `aws sts get-caller-identity --profile dd-codex` before running `kubectl`; if STS returns
  `InvalidClientTokenId`/expiry, refresh the profile rather than falling back to another auth path.
  It is **not** a WireGuard-VPN-plus-`dd-bastion` human-only step. (That bastion path still exists as
  a legacy fallback for private access and read-only kubeconfig retrieval, but is not required.)
- Browser access to protected public gateway paths goes through `dd-remote-auth`; configure
  the optional TOTP seed there when a passphrase plus one-time code is required.
- The legacy gateway auth header name is `Auth`; read its value from the operator secret or local
  environment when a human grants it. For local operator checks, use `ALL_DOGS` as the env var
  containing the `Auth` header value. Do not commit the literal value, print it in public docs, or
  echo it into browser-visible pages.
- The live-mutex broker (`dd-rust-network-mutex-raft`, `dd-rust-network-mutex`, `dd-live-mutex`) is
  reached through the gateway's `Auth`-header path (value from `ALL_DOGS`), so it intentionally does
  not set its own `LMX_AUTH_TOKEN` — do not treat that missing app token as an auth gap. The gateway
  authenticates external callers via `Auth`. In-cluster pod-to-pod traffic to the raft RPC (7980) and
  HTTP (6971) ports is not gateway-fronted and stays unauthenticated; restrict it with
  `dd-rust-network-mutex-raft.networkpolicy.yaml` if pod-level isolation is required.
- Public gateway paths must stay authenticated; avoid exposing MCP or bastion routes as
  unauthenticated Internet services.

## Local AWS Profiles

For local operator work that needs permanent AWS credentials, use the named profile in the human's
`~/.aws` files instead of copying key material into this checkout. The expected profile is
`dd-codex`: verify it with `aws sts get-caller-identity --profile dd-codex`, or set
`AWS_PROFILE=dd-codex` for commands that use the default AWS SDK/CLI credential chain. The profile
data lives in `~/.aws/config` and especially `~/.aws/credentials`; treat those files as
human-owned local state, not repo source. If STS validation fails for `dd-codex`, report the stale
or invalid local profile and stop AWS-mutating work until the profile is fixed. Never paste access
keys, secret keys, session tokens, or derived kubeconfig secrets into Git, agent prompts, generated
docs, or command output summaries.
