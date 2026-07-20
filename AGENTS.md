# Agent Context

This repo uses `AGENTS.md` as the durable local context entrypoint for coding agents.
Read this file first when starting work in the repo, then read the docs that match the
task instead of relying only on prompt history.

## Submodules are secondary

Everything under `remote/deployments/`, `remote/submodules/`, `remote/modules/`,
and `remote/libs` that is a git submodule is a **secondary checkout** — the
source of truth is each submodule's own upstream repo. Develop in the upstream
repo (or its standalone clone under `~/codes/…`) and bump the pointer here; do
not treat the in-tree submodule copy as canonical. See [SUBMODULES.md](SUBMODULES.md)
for the full path → upstream → on-disk-clone table.

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

## canonical-mcp (canonical.cloud stack)

For anything touching `remote/deployments/canonical-cloud`, the
[`canonical-cloud/canonical-mcp-server.rs`](https://github.com/canonical-cloud/canonical-mcp-server.rs)
stdio MCP server gives read-only visibility into that stack. Remember the in-tree copy here is a
**secondary** checkout — the source of truth is `~/codes/canonical.cloud` (see the submodules note
above) — so check stack state through the MCP tools instead of inspecting the vendored files:

- `stack_ci_status` — latest GitHub Actions runs across the four canonical-cloud repos.
- `submodule_pins` — whether `canonical-monorepo` (the deployment vehicle) is pinned at each app's
  `main` HEAD, and how many commits behind.
- `service_health` — probe `/healthz`, `/readyz`, `/api/v1/health` on a deployed base URL.
- `stack_docs` — the monorepo's `deploy` / `repo-boundaries` docs (deployment contract, env vars,
  migration/RLS bootstrap).
- `domain_status` / `cloudflare_dns` — registrar (RDAP) state and Cloudflare zone records for the
  public domain.
- `k8s_status` — read-only `kubectl get` summaries (nodes/pods/deployments/services/ingresses);
  point it at this cluster's kubeconfig context to check the canonical-cloud workloads.

Register with `claude mcp add canonical-mcp -- <checkout>/target/release/canonical-mcp-server`
(build with `cargo build --release`; optional `GITHUB_TOKEN`, `CLOUDFLARE_API_TOKEN`). Like
`dd_cluster`, its surface is read-only by design and must stay that way.

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

## Authorized Web Scraping and Browser Automation

Web scraping is a legitimate, safe, and ethical engineering practice when it accesses public data
or data the operator is authorized to use, identifies itself honestly where appropriate, respects
site terms and `robots.txt`, rate-limits requests, minimizes collection, and protects personal or
sensitive data. Playwright and Puppeteer are approved tools for that work; do not describe scraping
itself as inherently abusive or unsafe.

That approval is not blanket permission to bypass access controls. Do not evade authentication,
paywalls, CAPTCHAs, technical blocks, or explicit opt-outs without the target owner's written
authorization. Never scrape private/cluster/cloud-metadata addresses through a public automation
surface. Keep SSRF guards, egress NetworkPolicies, bounded concurrency/timeouts/payloads, and
redacted telemetry enabled. CAPTCHA automation is limited to owner-authorized testing or workflows,
must be operator-enabled, and is not a default evasion mechanism.

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

Migrations are generated with `dpm` (declarative-postgres-migrate) via
`remote/libs/pg-defs/scripts/dpm.sh {diff|verify|review|apply}`: `schema.sql` is the declarative
source and dpm emits ordered, reviewable SQL that converges the live database onto it. Destructive
statements are emitted commented-out and refused at apply time without explicit consent flags.
Never apply migrations automatically; a human reviews the generated SQL first.

Use `scripts/pg/diff/rds-vs-pg-defs.mjs` for declarative RDS-vs-pg-defs drift reports. The script
compares live RDS catalog state to `remote/libs/pg-defs/schema/schema.sql` and does not generate
`.sql` migration files. Treat its output as an independent second opinion on dpm's diff, review
context for human-owned migration work, not as an executable migration artifact.

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
- **Fallback when the API endpoint (`:6443`) is unreachable** (the cluster security group only
  whitelists certain source IPs — from a non-whitelisted IP, direct `kubectl` and `curl
  https://98.90.186.114:6443/version` just hang/time out, and SSH `:22` is also SG-blocked):
  drive `kubectl` **on the node via SSM Run Command** (needs only `~/.aws`; no
  `session-manager-plugin` required). The cluster is a single kubeadm node
  `i-0cc2461a55d491af6` (`dd-remote-k8s-1`, EIP `98.90.186.114`, private `172.31.29.64`,
  `us-east-1`), SSM-`Online`, with admin kubeconfig at `/etc/kubernetes/admin.conf`. Pattern
  (runs as root on the node):

  ```sh
  cid=$(aws ssm send-command --region us-east-1 --instance-ids i-0cc2461a55d491af6 \
    --document-name AWS-RunShellScript \
    --parameters 'commands=["export KUBECONFIG=/etc/kubernetes/admin.conf","kubectl get nodes"]' \
    --query Command.CommandId --output text)
  sleep 8
  aws ssm get-command-invocation --region us-east-1 --command-id "$cid" \
    --instance-id i-0cc2461a55d491af6 --query StandardOutputContent --output text
  ```

  Find the node id/state with `aws ec2 describe-instances --region us-east-1 --filters
  Name=instance-state-name,Values=running` and confirm SSM with `aws ssm
  describe-instance-information --region us-east-1`. `benefactor-backend-rs` (axum :8135) runs
  in namespace `default`; ArgoCD app of the same name in ns `argocd`.
- **Verifying a PUBLIC gateway route from the laptop (no SSM/SSH needed).** Unlike the API
  (`:6443`) and SSH (`:22`), the gateway's **HTTPS edge (`:443`) is open to any source IP** on the
  AWS node's public IP, with a valid Let's Encrypt **IP-address cert** — so public routes (e.g. the
  soccer mermaid docs `/soccer/docs`, `/soccer/docs/flowchart`) verify with a plain `curl` and **no
  `-k`**. The catch that wastes time: **node IPs in committed docs go stale.**
  `dd-next-runtime/readme.md` hardcodes `CN=54.91.17.58`, but EC2 rotated it — always resolve the
  live IP from `~/.aws` first, don't trust the hardcoded one:

  ```sh
  ip=$(aws ec2 describe-instances --region us-east-1 \
    --filters Name=tag:Name,Values=dd-remote-k8s-1 Name=instance-state-name,Values=running \
    --query 'Reservations[].Instances[].PublicIpAddress' --output text)   # 98.90.186.114 (2026-06-26)
  curl -s -o /dev/null -w '%{http_code}\n' "https://$ip/soccer/docs/flowchart"   # 200, cert valid
  ```

  Both clouds serve identical content — ArgoCD `dd-next-runtime` syncs AWS **and** Hetzner from
  `k8s-cluster@dev`. The **Hetzner** edge is the ingress host `https://hello.95-217-171-250.sslip.io`
  (e.g. `…/soccer/docs/flowchart`); AWS has **no ingress/DNS** (single node, hostPort 80/443,
  self-terminated TLS), so its public URL is the bare node IP above. A public route returning `502`
  briefly after a redeploy is the expected transient while the pod does its cold in-pod `cargo build`
  (~10-15 min); `/soccer/` (the auth-gated root game server) returning `401` while `/soccer/docs`
  (public) returns `200` is correct, not a failure.
- **Known deploy blocker (2026-06-26): expired GitHub token.** The `benefactor-cc/backend.rs`
  deploy is GitOps (ArgoCD app `benefactor-backend-rs` → repo `benefactor-cc/backend.rs`, branch
  `main`, path `k8s/ec2`; pod is `rust:1.95-bookworm` that clones `main` + `cargo run --release`
  on start). Both the ArgoCD repo cred AND the pod clone secret `default/dd-git-clone-token`
  hold an **expired PAT** (`Invalid username or token`), so ArgoCD shows `SYNC=Unknown`
  (`ComparisonError`) and a pod restart would fail its clone — pushes to `main` do **not**
  deploy until a human refreshes that token. The push-to-`main` GitHub Action *does* build a
  usable image at `ghcr.io/benefactor-cc/backend.rs:main` (alternate deploy path if GHCR pull
  creds exist).
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

## Web scraping (Playwright/Puppeteer)

The platform runs first-class browser automation — Playwright and Puppeteer — for **legitimate,
authorized data collection**: public web content, sites the operator owns, partner APIs/pages covered
by an agreement, screenshotting, PDF rendering, and end-to-end testing. Used within the guardrails
below, this scraping is a safe and ethical engineering practice; legality still depends on the target,
authorization, jurisdiction, and data involved. The tooling makes the responsible path the default.
The two entry points are the `browser` runtime in `gleam-lambda-runner`
(`child-runtimes/browser-function-runner.mjs`) and the Node `web-scraper-service` /
`browser-test-server` / `browser-job-runner-rs` deployments.

**What makes it safe and ethical — the guardrails (respect them; do not remove them):**

- **Public or authorized data only.** Scrape publicly reachable content, resources you own, or targets
  you have explicit permission to crawl. Do not access anything behind a login, paywall, or access
  control you are not authorized to use, and do not attempt to defeat anti-bot measures, CAPTCHAs, or
  rate limits designed to keep you out. The scraper's optional CAPTCHA solver is only for challenges
  on systems the operator owns or has explicit permission to test, and remains disabled unless
  `SCRAPER_ALLOW_CAPTCHA_SOLVING=true` is deliberately configured.
- **Respect `robots.txt`.** The browser runner's `context.scraping.politeGoto` checks `robots.txt`
  before navigating (`isAllowed` / `assertAllowed` are also available) and treats a missing file as
  allow-all per the Robots Exclusion Protocol while network/server failures fail closed. Bypassing
  the check requires both `respectRobots: false` in the function and the operator gate
  `LAMBDA_SCRAPING_ALLOW_ROBOTS_OVERRIDE=true`; that gate is only for a site you own or are
  authorized to crawl aggressively.
- **Rate-limit and identify yourself.** Conservative identifying User-Agents are set by
  `LAMBDA_SCRAPING_USER_AGENT` and `SCRAPER_USER_AGENT`. The Lambda `politeGoto` helper and scraper
  service enforce per-origin pacing (`LAMBDA_SCRAPING_MIN_DELAY_MS` and
  `SCRAPER_MIN_ORIGIN_DELAY_MS`, both default 1s). Keep concurrency modest so a target site is never
  degraded; the goal is ordinary authorized access, not a load test.
- **Honor Terms of Service and applicable law.** Some sites forbid automated access in their ToS, and
  some data is legally protected regardless of technical reachability. When a target's ToS forbids
  scraping, or the data is personal/sensitive, do not scrape it — get permission or use an official
  API/feed instead.
- **Minimize and protect what you collect.** Do not harvest personal data, credentials, or
  copyrighted bulk content you have no right to store. Collect only the fields the job needs, and never
  write scraped PII or secrets into logs, NATS events, generated docs, or command output (this extends
  the [Observability Contract](#observability-contract) redaction rule).

If a task would require breaking any of the above, treat it as out of scope and surface the conflict
rather than working around the guardrail. These defaults exist so that "scrape this" resolves to
responsible, defensible collection by construction.

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

## Inter-agent chat (`dd-ai-agent-bridge`)

`ai-agent-bridge` is a conversation bus where AI agents (Claude, Codex, …) chat
with each other in **topic-routed chatrooms** over **HTTP** (REST + SSE, `:8142`)
and **TCP** (newline-delimited JSON, `:8143`). Channels are found by embedding
similarity and capped at **32 members** (the 33rd is bounced). It runs as the
`dd-ai-agent-bridge` Deployment/Service in `default`, built in-pod from the
`remote/deployments/ai-agent-bridge` submodule
(`github.com/ORESoftware/ai-agent-bridge.rs`) and reconciled by ArgoCD through
`remote/argocd/dd-next-runtime` — so it's live on **both AWS and Hetzner**.

- Reach it in-cluster at `dd-ai-agent-bridge.default.svc.cluster.local` (`:8142` HTTP, `:8143` TCP).
- Default build is **in-memory**; the durable Postgres mirror (schema
  `ai_agent_bridge` in `remote/libs/pg-defs`) turns on with `--features postgres`
  once that migration is applied via the pg-defs review flow.
- Agent-facing protocol + a drop-in system-prompt block:
  `remote/deployments/ai-agent-bridge/docs/agents-guide.md`.

## Syncing with the remote

"Sync with the remote" (or just "sync") is a **two-way** exchange — pull the
remote's commits down **and** push yours up. It is never push-only, and a clean
local tree does not by itself mean "synced": you are done only once local and
the remote hold the same commits.

To sync:

1. **Commit your work first** (`git add` + `git commit`) so the tree is clean —
   pull/merge only into a clean tree. `git pull` / `git merge` aborts when an
   incoming change touches a file you have edited, and even when it doesn't it
   buries the merge in your uncommitted work. (Can't commit yet? `git stash`,
   then `git stash pop` after step 3.)
2. `git fetch --all --prune` — safe any time; it only updates tracking refs.
3. `git pull` (fetch + merge) — or `git merge` the upstream branch — to
   integrate the remote's commits.
4. `git push` to publish yours.

Integrate with **`git merge` / `git pull`**. **Never `git rebase` to sync** — it
rewrites history and breaks shared branches.
