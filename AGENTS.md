# Agent guidelines — canonical.cloud

> [!CAUTION]
> **Do not work in this checkout — use `~/codes/canonical-cloud/` instead.**
> The app subdirectories here are *mirrored copies committed as ordinary
> files*, not git repositories and not submodules (except
> `canonical-mcp-server.rs/`, which is a real submodule). Edits made here
> cannot be pushed to the app repos. See README.md.

Umbrella checkout for the **canonical.cloud** stack (GitHub org
`canonical-cloud`). Each subdirectory below has its own repo on GitHub, cloned
for real at `~/codes/canonical-cloud/<name>/` — work and push there:

- `canonical-monorepo/` — git superproject; submodule pins are the deployable
  release. **Deployment happens from here** (checkout + `./build.sh`
  reproduces the shipped stack; CI proves it with an integration build and a
  boot-the-stack smoke).
- `canonical-web-server.rs/` — Rust sMASH application server + TypeScript
  IndexedDB sync client.
- `canonical-marketing-site.web/` — Astro static marketing site.
- `canonical-interfaces/` — typed-IO source of truth (JSON Schema + SQL,
  generated language adapters).
- `canonical-mcp-server.rs/` — the MCP ops server described below.

The other three app repos exist for development and integration testing; they
never deploy on their own. This directory is also vendored as a secondary
submodule of `ORESoftware/k8s-cluster` (see README.md) — make changes here,
not there.

## canonical-mcp MCP server

`canonical-mcp-server.rs/` is a Rust stdio MCP server that gives agents
read-only visibility into the stack. Prefer its tools over guessing or
hand-rolling `gh`/`curl`/`kubectl` incantations:

| Tool | Use it for |
| --- | --- |
| `stack_ci_status` | Latest GitHub Actions runs across the four stack repos |
| `submodule_pins` | Is the monorepo pinned at each app's `main` HEAD? How far behind? |
| `service_health` | Probe `/healthz`, `/readyz`, `/api/v1/health` on a deployment |
| `stack_docs` | Fetch the monorepo's `deploy` / `repo-boundaries` docs |
| `domain_status` | Registrar (RDAP) + DNS delegation state for a domain |
| `cloudflare_dns` | List a Cloudflare zone's DNS records (needs `CLOUDFLARE_API_TOKEN`) |
| `k8s_status` | Read-only cluster state (nodes/pods/deployments/services/ingresses) via `kubectl get` |

Register it once (release binary is fastest):

```sh
cd canonical-mcp-server.rs && cargo build --release
claude mcp add canonical-mcp -- \
  "$PWD/target/release/canonical-mcp-server"
```

Optional env: `GITHUB_TOKEN`/`GH_TOKEN` (rate limits), `CLOUDFLARE_API_TOKEN`
(read-only zone token, required only for `cloudflare_dns`); `k8s_status` uses
your local `kubectl` and kubeconfig. Every tool is read-only — the server has
no write-capable GitHub, Cloudflare, or Kubernetes surface, and additions must
keep it that way.

## Command safety

Follow each subrepo's own `agents.md`; all of them blacklist destructive
shell commands (`rm -rf`, `git clean -fdx`, force-pushes to `main`, …) and
whitelist `git rm` / `git mv` so removals stay reviewable.

## Syncing with the remote

"Sync with the remote" (or just "sync") is **bidirectional and always contacts
the remote** — it pulls *and* pushes. It is never push-only, and a clean local
working tree does **not** by itself mean "synced": a sync is not finished until
local and the remote have exchanged commits in both directions.

The steps for a sync:

1. `git fetch --all --prune` — see what the remote has.
2. `git pull` (which merges) — or `git merge` the upstream tracking branch —
   to integrate the remote's commits into your local branch **first**.
3. `git add` / `git commit` any local work.
4. `git push` — publish your commits.

Always integrate with **`git merge`** (and plain `git pull`, which merges).
**Do not `git rebase`** to sync — rebasing rewrites history and breaks shared
branches; keep the merge history instead.
