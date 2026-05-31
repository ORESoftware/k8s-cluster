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
short-lived human grant, auth, and audit design. Treat minikube overlays as local mirrors only, not
as the live runtime source of truth.

## Command Safety

The following commands are blacklisted for agents in this repo: `git checkout`, `git reset`,
`git stash`, `rm`, and `sed`. Do not run them in local operator work or add them to automation
scripts. If a checkout has tracked dirty files, stop and surface the dirty state explicitly instead
of hiding it. Leave untracked runtime files alone unless a human asks for a specific cleanup.

Default branch posture for agent work is `dev`. Agents should not check out or switch to feature
branches for local operator work unless a human explicitly changes that posture for a specific task.
If the workspace is already on a non-`dev` branch, surface that state before doing branch-sensitive
work and prefer integrating feature work back into `dev` instead of continuing on the feature branch.

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

- External Secrets reads AWS Secrets Manager and syncs Kubernetes secrets.
- Agents receive only the strict env allowlist defined in `remote/deployments/dev-server/src/agents`.
- Humans use the WireGuard VPN plus `dd-bastion` for private cluster access and read-only
  kubeconfig retrieval.
- Browser access to protected public gateway paths goes through `dd-remote-auth`; configure
  the optional TOTP seed there when a passphrase plus one-time code is required.
- Public gateway paths must stay authenticated; avoid exposing MCP or bastion routes as
  unauthenticated Internet services.
