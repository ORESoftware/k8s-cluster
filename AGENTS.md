# Agent Context

This repo uses `AGENTS.md` as the durable local context entrypoint for coding agents.
Read this file first when starting work in the repo, then read the docs that match the
task instead of relying only on prompt history.

## Context Sources

- Read `docs/*.md` for cross-repo product or architecture notes.
- Read `agents/*.md` if that directory exists in a checkout. Treat it as agent-specific
  operating context, not application source.
- Read nested `AGENTS.md` files when working inside a subdirectory that defines one.
- For this repo, the highest-value runtime runbooks are:
  - `remote/readme.md`
  - `remote/dev-server/readme.md`
  - `remote/gleam-mcp-server/readme.md`
  - `remote/argocd/vpn/readme.md`
  - `remote/ec2/README.md`

## Runtime Context

Agents launched by `remote/dev-server` may receive selected Postgres context blobs in
the prompt. Treat those as task-specific memory, and treat this file plus the docs above
as persistent repo memory.

When the cluster MCP server is configured as `dd_cluster`, use it before guessing live
Kubernetes deployment state, service wiring, or observability status. The MCP surface is
read-only by default; do not add write-capable AWS or Kubernetes tools without a separate
auth and audit design.

## Access Posture

Do not put raw AWS keys, model keys, GitHub tokens, or gateway secrets in Git. The
preferred operator path is:

- External Secrets reads AWS Secrets Manager and syncs Kubernetes secrets.
- Agents receive only the strict env allowlist defined in `remote/dev-server/src/agents`.
- Humans use the WireGuard VPN plus `dd-bastion` for private cluster access and read-only
  kubeconfig retrieval.
- Public gateway paths must stay authenticated; avoid exposing MCP or bastion routes as
  unauthenticated Internet services.
