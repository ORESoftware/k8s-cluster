# Platform hardening — open follow-ups

Cross-cutting fixes and shore-ups surfaced while adopting `dpm` for Postgres
migrations, hardening `remote/libs` (`k8s-libs-and-shared-defs`), extending the
cluster MCP server, and overhauling CI (Jul 2026). Companion to
[`aws-learning-cluster-followups.md`](./aws-learning-cluster-followups.md)
(single-node operational items) and the security backlog in
[`../todos.md`](../todos.md) — this file does not duplicate those; it links them
and adds what is new.

Reach for cluster items: AWS API is not laptop-reachable — use SSM
(`aws ssm send-command --instance-ids i-0cc2461a55d491af6 --document-name
AWS-RunShellScript ...`) or the read-only `dd_cluster` MCP server. Durable
changes go through this repo's `dev` branch (argocd self-heals), not `kubectl`.

Status key: 🔴 blocking · ⚠️ open · 🟡 decision needed · ✅ done (kept for context).

---

## P0 — blocking

### 1. 🔴 `REMOTE_DEV_GH_PAT` is expired → `repo checks` + `pg-defs` CI red

The fine-grained PAT (`repo-checks.yml` uses it to init the private `remote/libs`
submodule) last rotated **2026-05-19** and has expired; the last several
`repo checks` runs on `dev` fail at "Initialize required contract submodule" with
`Invalid username or token`. This is the sole cause of the red CI, not a code
regression.

**Shore up:** mint a new fine-grained PAT with **read** access to
`ORESoftware/k8s-libs-and-shared-defs` **and** its nested `async-java` submodule
repo, then:
```sh
gh secret set REMOTE_DEV_GH_PAT -R ORESoftware/k8s-cluster
```
Consider a calendar reminder to rotate before the next expiry, or switch to a
GitHub App installation token (no hard expiry) for submodule reads.

### 2. ⚠️ Uncommitted hardening edits in the working tree

Two reviewed fixes are sitting in the `dev` working tree, never committed (the
branch has been actively moved since):

- `.githooks/pre-commit` — gitleaks scan of the staged diff (same rules as
  `secret-scan.yml`), degrades to a warning if gitleaks is absent, `--no-verify`
  bypass documented. **Also register it:** it only runs if
  `git config core.hooksPath .githooks` is set (add that to `.githooks/install.sh`
  so it is applied on clone/setup).
- `remote/deployments/build-server-rs/scripts/dpm.sh` — passes the target DB URL
  to `dpm` via `TARGET_DATABASE_URL` env instead of `--target` on argv (keeps the
  password out of `ps`/procfs), plus the pinned-installer hint. Mirrors the fix
  already merged in the libs `pg-defs/scripts/dpm.sh`.

**Shore up:** commit both to `dev` with the current app work, or cherry-pick them
onto a dedicated branch so they are not lost on the next reset.

---

## P1 — security (see also [`../todos.md`](../todos.md))

### 3. ⚠️ `build-server` static AWS keys → instance-role / IRSA

The only kube-secret-backed AWS key path in the repo. Details and the concrete
migration in [`../todos.md`](../todos.md) §1. Now unblocked infra-wise: the build
server got its own declarative DB contract
(`remote/libs/pg-defs/schema/databases/dd_build_server/schema.sql`, migrated via
its own `dpm.sh`), so the credential swap can land alongside.

### 4. ⚠️ Block IMDS for host-network warm workers

Host-network worker containers can reach `169.254.169.254` and lift the node
instance-role credentials. The only fix that covers host-network pods is an
`nftables` drop rule on the EC2 host for `dst 169.254.169.254` from non-kubelet
UIDs. Full options in [`../todos.md`](../todos.md) §2.

### 5. ⚠️ `remote-k8s-maintenance.yml` inlines secrets into SSM command parameters

`sync-agent-gh-pat` and `sync-agent-model-keys` interpolate the PAT / model API
keys directly into the SSM `commands` text. SSM Run Command stores parameters in
plaintext, retrievable for 30 days via `ssm:GetCommandInvocation` and possibly
logged to CloudTrail/S3.

**Shore up:** write the secret to Secrets Manager / SSM Parameter Store
(SecureString) from the runner and have the on-node script fetch it by name, so
the SSM command carries only the parameter name. While there, default the
workflow token to `contents: read` and give the one result-push operation
(`verify-mip-solver-node`) its own narrowly-scoped job.

### 6. ⚠️ Credential rotation still owed

STS creds pasted into chat (2026-05-23), account root access keys, and the
over-broad `my-cli-user` (`AdministratorAccess`) — rotate/scope per
[`../todos.md`](../todos.md) (§ Reminder and §C). Human-only.

### 7. ⚠️ Transcript-redaction sweep

Persisted `agent-transcripts/*.jsonl` and `tmp/convos/thread.log` are not run
through `sanitizeEventText`. [`../todos.md`](../todos.md) §3.

---

## P1 — cluster MCP server (`cluster-mcp-rs`): code shipped, deploy decisions pending

The Rust server was hardened (bearer gate on all non-probe routes, constant-time
token compare, annotation/managedFields stripping + value-pattern redaction,
`isError`, source-IP logging, protocol negotiation) and gained read-only
integrations (`cloudflare_zones`/`_dns_records`, `domain_registration` via RDAP,
`domain_dns_wiring` via DoH, `kubernetes_ingress_endpoints`,
`deployment_rollout_status`, `kubernetes_events_warnings`). What remains is
deploy-side and needs an operator decision:

### 8. 🟡 Turn the bearer gate on

`MCP_REQUIRE_AUTH` defaults to `false`; the in-VPC `:8090/:8091` ports accept
unauthenticated calls (RFC1918 NetworkPolicy rule for host-network warm workers).
**Shore up:** populate `MCP_AUTH_SECRET` in the existing ExternalSecret pipeline,
set `MCP_REQUIRE_AUTH=true`, and hand the token to the pool-worker launcher env.

### 9. 🟡 Narrow the NetworkPolicy ingress

`172.31.0.0/16` admits every ENI in the VPC. Warm workers originate from the
node's own host IP — narrow the `ipBlock` to that address/32 (or the node
subnet). Defence-in-depth for #8.

### 10. 🟡 Prebuilt image instead of build-at-boot

The pod runs `rust:1.90-bookworm` and `cargo run --release` against a hostPath
checkout (512Mi–4Gi, ~10-min startup budget, a live `0.0.0.0/0:443` egress rule
so it can fetch crates). The production multi-stage `Dockerfile` is checked in but
unused. **Shore up:** build/push it, point the Deployment at it, drop the egress
rule, cut resources to ~64–256Mi. **Note:** the new Cloudflare/RDAP/DoH/k8s tools
only run once the image is rebuilt from the current source.

### 11. 🟡 Retire the Gleam MCP server?

`gleam-mcp-server` duplicates the same 13-tool surface at a second gateway path,
with two of its tools returning hardcoded data and a sequential (~60s worst-case)
inventory fan-out. The Rust server is a functional superset. Consider porting its
one unique feature (NATS `McpToolEvents` audit publish) to Rust and retiring the
Gleam app + its RBAC/NetworkPolicy/gateway location.

---

## P2 — single-node operational resilience

Most items live in [`aws-learning-cluster-followups.md`](./aws-learning-cluster-followups.md)
and [`../todos.md`](../todos.md) (GCS loadtest section). Cross-cutting reminders:

### 12. ⚠️ Build-at-boot is the recurring root cause

Multiple services (`dd-remote-rest-api`, `cluster-mcp-rs`, fabrication, queue
consumers) compile Rust at pod start on the single 14-vCPU node. Consequences
seen this effort: `container pool config e2e` "fails" only because rollouts
outlast its wait window; CPU saturation impairs the node/SSM; slow recovery after
relaunch. **Shore up:** prebuilt images per service (same as #10), rolled out
incrementally.

### 13. ⚠️ No stable endpoint for the `:6443` API

After an instance relaunch the kubeconfig can point at a dead ephemeral IP (the
gateway EIP `98.90.186.114` is a separate concern). **Shore up:** associate a
stable EIP (or NLB/DNS) for the API endpoint. [`../todos.md`](../todos.md) GCS §2.

### 14. ⚠️ Pod graveyard accumulates

`usacc-rest-api-backend-rs` had ~25 `Evicted`/`ContainerStatusUnknown` pods
lingering up to 16 days (cleared once this session with
`kubectl delete pods -A --field-selector status.phase=Failed`). **Shore up:** a
periodic reaper (CronJob or a `remote-k8s-maintenance` operation) for
`status.phase=Failed` pods, and fix the underlying eviction (CPU/memory pressure,
tied to #12).

### 15. ⚠️ Intermittent pod→apiserver path degradation — watch

The MCP pod has timed out reaching `kubernetes.default.svc` while in-cluster
observability targets answered in ~3ms; `dd-k8s-resource-exporter` crashlooped 13×
during a config rollout window (self-recovered, stable since). No Cilium policies
are applied yet, so it is not policy-related — likely kube-proxy/VIP path or
apiserver overload under CPU pressure on the single node. **Shore up:** if it
recurs, capture `kubectl -n observability logs --previous` for the exporter and
check apiserver latency; the durable fix is reducing node CPU pressure (#12).

---

## ✅ Done this effort (context, no action)

- **dpm adopted + convergence bug fixed and released.** `dpm` v0.3.2 canonicalizes
  CHECK/index/generated-column/view defs to their re-parse fixed point
  (`declarative-postgres-migrate.rs` `3fcb17b` + `16e2c9b`). The libs CI
  `dpm verify` step is now **enforcing**, and each `schema/databases/*` per-DB
  contract is validated. See `remote/libs/pg-defs/readme.md`.
- **libs hardening:** `diff.mjs` `--env` path-traversal containment + test;
  `dpm.sh` keeps the DB password off argv; per-database schema CI coverage.
- **CI overhaul:** four duplicate dd-dev-server image workflows consolidated;
  all four `remote/libs` generators drift-checked; kustomize auto-discovery
  (22 overlays gained coverage); push+PR double-runs removed; timeouts/concurrency
  added; dependabot + `nix flake check` workflows added.
- **`nix/` → `.nix/`** (flake + `.envrc` updated).

## Local repo hygiene

`dev` has been diverging from `origin/dev` repeatedly this month (currently ahead
1 / behind 6). Reconcile regularly — per `AGENTS.md`, integrate feature work back
into `dev` and never `git stash`; commit to a branch to set work aside.
