# Platform discovery backlog — 2026-07-18

A deep, adversarial discovery pass across all ~28 fiducia.cloud repos (7 parallel
deep-dives: coordination core, messaging/sync, auth+web, agent plane, contract
drift, infra/observability, test coverage). The surface is mature — **no
`unimplemented!`/`todo!` panics anywhere** — so these are traced correctness bugs,
authority gaps, and design debt, each with file:line evidence and a concrete first
step. CONFIRMED = a code path was traced end-to-end; SUSPECTED = needs live repro.

**Already fixed this pass:** the P0 MFA bypass (C1 below) — `fiducia-customer.rs@d755ecf`.

**Also fixed (2026-07-18 tier-2 emulation + test-hardening pass):** the ops-CP rollout
crash-resume data-integrity bug — `fiducia-operations-control-plane@3254b03`. It was a
sibling of **M4** (same complete-before-effect pattern). See **Addendum A** for that fix,
a **live reproduction of the H10 blackhole class** in the local 3-cluster emulation, and
the tooling/testing gaps that surfaced running the fleet end-to-end.

---

## P0 — Critical (data-loss / auth-bypass / split-brain), do first

### C1 — Password login bypassed TOTP MFA ✅ FIXED (`fiducia-customer.rs@d755ecf`)
`customer_login_submit` issued the session cookie straight after the Supabase
password grant with no factor lookup; only the OTP path enforced step-up. Any
MFA-enrolled account (or anyone with just the password) skipped the second factor
via the always-rendered password form. Fixed by running the same fail-closed
`list_factors`→`required_totp_factor`→`begin_mfa_step_up` branch before issuing any
cookie; regression test added. Corroborated independently by the auth and
test-coverage dives.

### C2 — Node read paths mutate replicated state + mint fencing tokens off the log — CONFIRMED
`fiducia-node.rs/src/state.rs:2291,2301,2409,3354` via `consensus.rs:2342 handle_query_local`.
An inventory read (`LockInventory`/`SemaphoreInventory`/`ElectionList`) landing on a
**follower** runs `expire_due(now)`, promotes a queued waiter, and **mints a fencing
token on that follower alone** — not in the Raft log. Consequences: (a) follower
state diverges from deterministic replay (a later snapshot carries the phantom
mutation); (b) two nodes mint the **same token value for different holders** →
fencing-token reuse / split-brain. First step: make inventory/`*_get` expiry-*aware
but pure* (compute "live at now" without mutating `self`), mirroring `record.view`;
all promotion/minting stays in `apply_at` under `proposed_at_ms`. Effort M.

### C3 — Brain reconcile wipes every shard to empty replicas on total node loss — CONFIRMED
`fiducia-brain.rs/src/scheduler.rs:181-184,262-291,123-129`. When the healthy set
empties while nodes remain known-Dead, the shrink guard (absent-only) passes and
`plan_replicas(...,[],rf)` returns `[]` for every shard; the empty map commits and is
served at `GET /v1/placement` → total fleet unavailability, then re-place-from-scratch.
First step: before the per-shard loop, if `healthy_ids.len() < rf` treat the tick as
incomplete-membership and never propose a replica set smaller than
`min(current.len(), rf)`. Effort S. **Highest value-to-effort ratio in the backlog.**

### C4 — Bridge file leases are RAM-only; fencing counter resets to 0 on restart — CONFIRMED
`fiducia-ai-agent-bridge.rs/src/state.rs:121,152,153`; no `file_leases` table in `db.rs`.
Leases live only in a `RwLock<HashMap>` and the counter is `AtomicU64::new(0)` with no
hydration — so leases are non-durable across restart *even with Postgres present*, and
tokens are **non-monotonic across restart**: a token-5 holder pre-restart is later
undercut by a token-1 holder, inverting the fencing check. First step: persist the
counter high-water and restore `next = max(persisted)+1` on boot; add a `file_leases`
table. Effort M.

---

## P1 — High

| # | Item | Evidence | First step | Eff |
|---|------|----------|-----------|-----|
| H1 | Brain failure detection uses wall-clock, not monotonic → NTP jump / VM migration > `dead_after_ms` marks the whole fleet Dead (triggers C3) | `fiducia-brain.rs/src/scheduler.rs:334-339`, `membership.rs:184`, `api.rs:111-116` | track liveness on `Instant`; keep wall-clock only for display | S |
| H2 | Sync deleted rows resurrect — no tombstone; a `Delete v6` then stale `Upsert v5` re-creates the row (tests enshrine it as correct) | `fiducia-sync/src/lib.rs:93-96`, `sdk/src/{client,store}.mjs`, `tests/core_edge.rs:112` | keep a tombstone (deleted-id→version) consulted in reconcile | M |
| H3 | Broker `duplicate_window` never verified; crate default needs 600s vs JetStream default 120s → silent double-delivery on crash-window republish | `fiducia-messaging.rs/src/outbox.rs:40,45,58`, `main.rs`, `publisher.rs:134` | assert `stream.duplicate_window >= min_duplicate_window` at relay startup, fail closed + document | S-M |
| H4 | `ensure_consumable`/`require_fencing_token` — the effectively-once consume gate — is called by **zero** consumers | `fiducia-messaging.rs/src/envelope.rs:218-264`; grep of lambda/agent-manager | wire the gate into each mutating consumer; add a consume helper that refuses the payload until it passes | M-L |
| H5 | Lambda "durable" workflow store is in-memory only → runs lost on restart, not shared across replicas (voids the run-lease design) | `fiducia-lambda-service.rs/src/workflow/store.rs:88-89,172`; false claims `engine.rs:6`, `README:15` | implement the Postgres store the contract describes, or correct all durability claims | L |
| H6 | Lambda run-lease is fail-**open** with no coordinator (default) → double-executed activity side effects | `fiducia-lambda-service.rs/src/coord.rs:153-161`, `config.rs:132`, `engine.rs:408` | per-run inflight lock; enforce fencing token at store commit | M |
| H7 | Lambda host runtimes have no CPU/mem/pid/egress sandbox (default on); NATS creds forwarded into function env | `fiducia-lambda-service.rs/src/runtime.rs:232,237`, `config.rs:154`, `child_runner.rs:312` | stop forwarding NATS URL into function env; default host runtimes empty (require container); egress allowlist | M |
| H8 | Lease-steal: bridge/CP return `fencing_token` in read-only holder lookups + authorize on a self-asserted `agent_key` | bridge `types.rs:57,65`→`http.rs:518`, `state.rs:363-397`; CP `main.rs:161` | stop publishing `fencing_token` in lookups; bind lease owner to the authenticated actor (see identity design) | M |
| H9 | Agent-manager: concurrent sessions share one physical checkout (repo bind mode), serialized only per-session → interleaved git ops, wrong-branch commits | `fiducia-ai-agent-manager.rs/src/orchestrator.rs:49,231`, `state.rs:31`, `http.rs:190` | single per-workspace async mutex held prepare→push | M |
| H10 | Brain member id / node id set to non-dialable `$(POD_NAME).$(CLUSTER)` → cross-cluster Raft forwarding + client redirects blackhole | infra `base/components/brain/statefulset.yaml:67`, `base/node/statefulset.yaml:97` vs code contract `brain/main.rs:145`, `node/consensus.rs:278` | set `FIDUCIA_BRAIN_ID`/`NODE_ID` from `topology.toml` `*_endpoint` (dialable host:port) | S |
| H11 | Brain placement `generation` not monotonic across restart/failover/snapshot → a poller comparing `>` ignores the new authoritative map | `fiducia-brain.rs/src/placement.rs:22-26,96`, `main.rs:107`, `raft_driver.rs:678` | derive generation from Raft commit index; confirm poller uses `!=` not `>` | M |
| H12 | Auth mints JWTs with no `aud`, but edge + LB both require `aud="fiducia-api"` → fiducia-issued tokens rejected | `fiducia-auth.rs/src/token.rs:36-44` vs `fiducia-edge/src/index.mjs:584`, `fiducia-load-balance.rs/src/auth.rs:228` | add `aud` to Claims/`mint_with`, or relax validation for the fiducia issuer (confirm which token flows) | S |
| H13 | No `aal` claim ever checked (root cause of C1) — MFA enforced only by flow control; any aal1 token in the session cookie is trusted | `fiducia-auth.rs/src/supabase.rs:106-118`; grep `aal`=∅ | thread factor/`aal` into `UserCtx`; reject aal1 sessions for MFA-enrolled accounts | M |
| H14 | MFA factor disable/enroll requires no step-up/re-auth → password-only attacker (post-C1) strips victim's authenticator | `fiducia-customer.rs/src/main.rs:1360-1390` | require a fresh aal2 (current TOTP) before any factor mutation | M |
| H15 | Semaphore acquire contract drift: schema/generated say `max` on `/v1/locks/{key}/acquire`; server implements required `limit` on `/v1/semaphores/acquire` | `fiducia-interfaces/schema/locks.schema.json:11` + `generated/rust/src/lib.rs:994` vs `fiducia-node.rs/src/semaphore.rs:12` | pick source of truth (server won); fix schema + regenerate | M |
| H16 | Prod Terraform state is local & unencrypted (no `backend.tf`) → first apply writes k3s tokens + kubeconfigs/CA to a plaintext, unlocked file | `fiducia-infra/terraform/envs/prod/backend.tf.example` only | wire S3+DynamoDB/GCS backend, `init -migrate-state` | S |
| H17 | Brain StatefulSet has no PodDisruptionBudget (node & LB have one) → draining the sole member has no eviction gate → Raft quorum loss | `fiducia-infra/base/components/brain/` (no pdb.yaml) | add `maxUnavailable:0` PDB | S |

---

## P2 — Medium (correctness / robustness / hardening)

- **M1** Compat outbox relay holds a DB tx across NATS publish+flush for the whole batch (lock-across-IO); crash re-publishes the sent prefix — `messaging/src/transactional.rs:112,148,177`. → commit-claim / publish-outside-tx / mark-per-row. S-M
- **M2** Compat relay unbounded retry, no dead-letter/park — `messaging/src/transactional.rs:20,96`. → add `max_attempts` + park. S-M
- **M3** `inbox_try_insert` (pool path) claim-before-effect silent-effect-loss, used by real consumers — `messaging/src/db.rs:345`. → `#[deprecated]` toward `PgInbox`; audit 3 call sites. M
- **M4** Scheduler `claim_due` completes idempotency before dispatch → crash drops a scheduled run — `operations-control-plane/src/scheduler.rs:90-108`. → split claim from complete. M **(STILL OPEN — this is the exact pattern fixed in the rollout path at `@3254b03`; the scheduler is the last `complete-before-effect` site in this repo. Apply the same claim→dispatch→complete reorder; see Addendum A.)**
- **M5** No integrity checksum on any persisted file (node + brain) + brain's one-directional base_index validation — `node/persist.rs`, `brain/raft_store.rs:26,165`. → blake3 header verified on load. M
- **M6** `peers` never de-duplicated → inflated quorum / commit-without-quorum (node) and no-leader-electable (brain) — `node/consensus.rs:296`, `brain/raft.rs:323`. → dedup + self-exclude at parse. S
- **M7** Brain follower splices AppendEntries with no contiguity/base-index check (node validates; brain doesn't) — `brain/raft.rs:588`. → reject gapped/at-or-below-base entries. S
- **M8** Brain leader trusts follower-reported `match_index` instead of deriving it — `brain/raft.rs:606,625,800`. → `min(resp.match_index, last_sent)`. S
- **M9** Brain committed-entry truncation guarded only by `debug_assert!` (compiled out in release); commit_index not re-clamped — `brain/raft.rs:719`. → hard runtime guard + re-clamp. S
- **M10** Brain `raft_store` silently truncates the log at the first unparseable record (drops all after) — `brain/raft_store.rs:85`. → port node's torn-vs-corrupt distinction. S-M
- **M11** Node unbounded map growth — tasks/effects/handoffs/decisions/barriers/rate_limits never GC'd → snapshot bloat / OOM — `node/state.rs:2409`. → committed retention sweep. M
- **M12** Brain Dead nodes never forgotten (tombstone leak; enables C3) — `brain/scheduler.rs:297`, `membership.rs:177`. → `ForgetNode` after grace + no shards. S
- **M13** Brain scale-down trims by load only, ignoring failure domains → anti-affinity violation — `brain/scheduler.rs:341`. → preserve ≥RF distinct domains. M
- **M14** Agent-manager warm checkout never `git clean`ed → untracked files leak across work-items into wrong PR — `agent-manager/src/orchestrator.rs:170`. → `git clean -xdf` around switch. S
- **M15** Agent-manager `safe_repo_relative` doesn't resolve symlinks → path-escape on deterministic-append — `agent-manager/src/orchestrator.rs:519`. → canonicalize + re-assert containment. S
- **M16** Agent-manager TOCTOU in dispatch dedup → same `task_id` spawns two runs — `agent-manager/src/http.rs:219,272`. → single check-and-insert critical section. S
- **M17** Bridge dead subscribed TCP conns leak slots + HOL-block (no server keepalive/write timeout) — `bridge/src/tcp.rs:144,112`. → periodic ping + write timeout. M
- **M18** kubelet-probe NetworkPolicy opens sidecar `/meta`+`/metrics` (:8091) and otel (:13133) to any pod, no `from:` selector — `infra/base/networkpolicy.yaml:104-115`. → restrict source / add L7 guard. S-M
- **M19** vultr prod VKE has no API allowlist (hetzner/civo fixed) → one prod cluster's k8s API public — `infra/terraform/modules/vultr/main.tf`. → firewall group + world-open precondition. M
- **M20** No alerting/SLO rules anywhere (no PrometheusRule/Alertmanager) → quorum loss pages nobody — `infra/base/*`. → minimal SLO alerts for the coordination core. M
- **M21** Core images pinned by mutable git-SHA tag, not `@sha256:` digest — `fiducia-monorepo/.github/workflows/deploy.yml:110`. → resolve+write digest. S
- **M22** otel span-attr redaction misses `x-fiducia-*` trusted-hop headers — `infra/base/observability/otel-agent.yaml:120`. → add header keys / regexp redaction. S
- **M23** Brain ignores WAN-tuned `FIDUCIA_RAFT_*` timings + `FIDUCIA_REPLICATION_FACTOR` (node reads them; brain hardcodes) — `brain/raft_driver.rs:42`, `config.rs:27`. → read the envs or drop from brain surface. M
- **M24** Barrier/Decision policy: schema marks tuning fields optional; node's tagged-enum variants make them mandatory (+ i64/u32 signedness) — `interfaces .../lib.rs:189` vs `node/state.rs:614`. → align optionality + width. S
- **M25** fiducia-memory reuses `/v1/claims/*` paths with an incompatible contract (no `name`, `valid_until_ms` vs ISO, no `generation`) — `memory/src/main.rs:145`. → namespace memory's endpoints or converge types. M
- **M26** Two dead auth flags (`FIDUCIA_JWT_ISSUER`/`AUDIENCE`, live in LB/edge but read by nothing in auth; auth reads `SUPABASE_AUTH_*`) — `auth/.cli-flags.toml:44-54`. → delete or wire. S
- **M27** Secrets missing from flags-2-env `[env].ignore`: `FIDUCIA_KV_ENCRYPTION_KEY` (node), `CUSTOMER_API_KEY_PEPPER` (auth), `FIDUCIA_ALLOW_INSECURE_INTERNAL` (memory). → add to ignore. S
- **M28** Lambda signals at-least-once, no dedup; run-start idempotency only 60s; unbounded retention of signal-blocked runs — `lambda/src/workflow/store.rs:176,247`. → signal dedup id; align TTLs; sweep non-terminal. S-M
- **M29** RW-lock client methods ship in every generated client but no service serves `/v1/rw/*` → 404 — `fiducia-clients/clients/*` vs `fiducia-node.rs` (no rw router). → implement in node or remove from generator. M

---

## P3 — Test-coverage gaps on critical paths (highest-risk, all "genuinely untested")

1. Node: in-flight proposal drain on leadership loss (`fail_pending` resolves exactly once as NotLeader) — `consensus.rs:1267`.
2. Node: fencing-token monotonicity *after* snapshot→restore (existing test asserts structure only, not that the next mint exceeds the pre-snapshot max) — `state.rs:2316,1718,6085`.
3. Node: InstallSnapshot/boot **rejection** of an invariant-violating snapshot (unit validator tested; integration wiring not) — `consensus.rs:1645`.
4. Brain: `AssignShard` never travels through Raft commit/snapshot in any test (all use `SetScalePlan`); `placement.rs` has no test module — `cluster.rs:115`, `raft_driver.rs:661`.
5. Brain: `reconcile()`/`plan_replicas` anti-churn fixed point + at-most-one-replica bound (oscillation → perpetual fleet data movement) — `plan.rs:95`.
6. Auth: JWKS verification has zero real-signature coverage (every test passes `"junk-jwt"`; rotation-overlap + retired-kid + wrong-iss/aud unasserted) — `supabase.rs:83`.
7. Auth: KV CAS retry **exhaustion** (`CasRetriesExhausted`) never asserted — `keys.rs:249,323,466`.
8. Messaging: `PgInbox` (`inbox.rs`) has **zero** tests — the no-lost-effect guarantee — `inbox.rs:66,87`.
9. Messaging: DB ownership-CAS / SKIP-LOCKED concurrency verified only by SQL-string matching, never executed — `db.rs:196,227`.
10. Cross-repo: trusted-hop header contract (JS edge ↔ Rust LB) — each side tests its own copy of the literal; no cross-check — `edge/index.mjs:525` vs `lb/auth.rs:346`.
11. Customer: aal1-token-on-protected-route rejection (the C1/H13 invariant) — no unit test — `customer/auth.rs:122`.
12. Auth: alg-confusion / symmetric-JWK / alg=none negative tests (defended today; no regression guard) — `supabase.rs:348`, `token.rs:157`.

---

## Cross-cutting: per-actor identity + operator/service role split (the #1 architectural item)

One `FIDUCIA_INTERNAL_SECRET` authorizes all internal `/v1`, with no per-actor identity
or operator-vs-service split; leases trust a self-asserted `agent_key` and leak the
`fencing_token`; lambda `/invoke` has no tenant claim. There is an in-repo precedent to
generalize: agent-CP's `authenticated_reviewer` credential registry
(`fiducia-ai-agent-control-plane/src/main.rs:748-814`). Phased, dual-accept, ship-each-phase plan:

- **Phase 0** — add an `Actor{id,role,scopes}` type + a dual-accept extractor (still accepts the shared secret as a synthetic `legacy/service` actor; also accepts `x-fiducia-actor-auth` against a new credential registry). Pure addition, zero behavior change.
- **Phase 1** — split `Role` into operator/service/agent; gate operator-only endpoints (brain `scale`/`nodes`/`policies`, lambda admin `workflows/*`, agent-manager manual PR/commit/merge) on `role==operator`. Rollback = widen the check.
- **Phase 2** — bind lease ownership to the authenticated actor (not body `agent_key`); stop returning `fencing_token` in read-only lookups (fixes H8).
- **Phase 3** — replace static per-actor secrets with short-lived signed tokens (`sub,role,scope,org,exp`); the `org` claim gives lambda `/invoke` its first tenant scope.
- **Phase 4** — retire the shared secret once all clients present actor credentials.

Staging invariant: additive env first (flags-2-env), then a per-env boolean flips
enforcement, then legacy removed last — so no synchronized cross-service cutover.

---

## Addendum A — 2026-07-18 tier-2 emulation + test-hardening pass

This pass added ≥2 behavioural tests to every active repo (54 new tests, all green,
pushed) and stood the **local 3-cluster emulation** (`fiducia-infra/kind/multicluster`)
up end-to-end for the first time — three separate Kind clusters, cross-cluster Raft,
WAN-latency + partition injection. Running the real fleet (not just unit tests)
confirmed several backlog items live and surfaced a few new ones.

### A1 — FIXED: ops-CP rollout was permanently un-resumable after a coordinator crash (data integrity)
`fiducia-operations-control-plane/src/workflows.rs` `run_rollout`, fixed at `@3254b03`.

Root cause — the two-phase idempotency contract is: `claim_idempotency` returns
`Ok(false)` for a *claimed-but-uncompleted* key and `Err(AlreadyCompleted)` for a
*completed* one. This holds for **both** `MemoryCoordination` **and the production
`FiduciaCoordination`** (which maps fiducia-node's completed idempotency record to
`AlreadyCompleted` — verified in `src/fiducia_coordination.rs`). `run_rollout` drove
each batch through `fenced_effect`, which (a) **completed the key *before*
`executor.deploy` ran** and (b) **propagated `Err(AlreadyCompleted)` via `?`**. So the
instant any batch finished, its key was completed — and a replacement coordinator
re-invoking `run_rollout` hit `AlreadyCompleted` on the first done batch and aborted
the entire resume. The advertised dedup path (`!fresh => continue`) only ever saw
`Ok(false)`, a state the natural flow never produced. Net effect: the crash-recovery
the module is *built around* did not work — a coordinator that died mid-rollout left
the operation impossible to resume.

The fix mirrors `run_migration` (which was already correct): **claim → deploy →
complete** (complete only after the batch lands and clears its health barrier, so a
crash mid-deploy leaves the key *claimable* rather than marking a never-deployed batch
done); treat **both** `Ok(false)` and `Err(AlreadyCompleted)` as "skip this batch"; and
make the final transition idempotent (guard `Completed→Completed`) so re-invoking a
finished rollout is a clean no-op. Two regression tests exercise the *natural* resume
the prior test missed (it injected a claimed-but-uncompleted key; a batch that actually
finished is *completed*).

**Follow-up (open):** `fenced_effect` is now used only by a test — it is a `pub` method
whose sole remaining caller is `tests/rollout_failover.rs`. Either delete it, or if it
is meant as an external primitive, its complete-immediately-after-claim shape is the
wrong bracketing for any effect that can crash mid-flight (same lesson as M4). Decide
and either remove or re-shape it. Effort S.

### A2 — CONFIRMED LIVE: H10 non-dialable / stale peer address blackhole
H10 (brain/node id set to non-dialable `$(POD_NAME).$(CLUSTER)`) was static-analysis
"SUSPECTED"; the emulation reproduced its **class** live. After a host/Docker restart,
Kind container IPs change but the deployed `fiducia-cluster` ConfigMap keeps the old
peer IPs, and the pods snapshot env at start — so one cluster's LB returned
`502 {"error":"no_leader","detail":"exhausted redirects/retries"}` for every write
while its route table showed all 16 leaders (it had leaders, it just could not *reach*
them). Raft still converged through the one correct peer, which is exactly why the
failure is silent at the consensus layer and only visible at the request path. This is
the same failure mode H10 predicts for prod when the id is a non-dialable name. The
prod fix is H10 (dialable `host:port` from `topology.toml`); the emulation fix is A3.

### A3 — NEW (tooling): `multicluster/up.sh` reuse path doesn't restart pods after rewriting peer config
`fiducia-infra/kind/multicluster/up.sh`. On a re-run it re-discovers container IPs and
rewrites each cluster's `topology.env`/ConfigMap, but does **not** roll the
statefulsets/deployments — so pods keep the stale peer env until a manual
`kubectl rollout restart`. That is what makes A2 sticky across a Docker restart. Fix:
after applying the refreshed ConfigMap, `kubectl --context … rollout restart
statefulset/fiducia-node statefulset/fiducia-brain deployment/fiducia-load-balance` (all
three clusters) and wait for readiness. Effort S. Also worth a one-line troubleshooting
note in `kind/multicluster/README.md` so the next operator recognises the
`no_leader/exhausted redirects` symptom (not added there yet — infra had in-flight work
at the time of writing).

### A4 — Test capability added: the e2e client can now drive nodes directly (Tier-2 usable)
`fiducia-e2e/src/client.mjs` `@0b98177`. The conformance client gained (env-gated, off
by default so LB-fronted runs are untouched) trusted-hop headers
(`FIDUCIA_E2E_INTERNAL_SECRET` → `x-fiducia-internal-auth` + `x-fiducia-org-id`) and a
bounded NotLeader (`307`) failover across `FIDUCIA_E2E_ENDPOINTS` — a kind follower
redirects to a bridge IP the host can't reach, so following the redirect fails; retrying
the other endpoints is the SDK's documented behaviour anyway. With this, the full
conformance suite runs green against the live 3-cluster tier (58 pass / 0 fail / 4
undeployed-route skips). This also gives P3 item 10 (cross-repo trusted-hop contract,
"no cross-check") a live exerciser it lacked — worth promoting into CI against a
Tier-1 kind bring-up.

### A5 — Minor: OTLP batch-export error noise when no collector is reachable
Seen in LB logs under the emulation (no otel gateway deployed in the Raft-only slice):
`opentelemetry_sdk … BatchSpanProcessor.Flush.ExportError reason="ExportTimedOut(30s)"`
every export cycle. `fiducia-telemetry::init` already degrades correctly (traces are
best-effort), but the 30s-timeout error line is repetitive noise that could mask real
errors in a log scan. → shorten the batch export timeout and/or log the first export
failure then suppress until recovery (mirror the sidecar's scrape-failure escalation).
Effort S.

## Themes for whoever picks this up

1. **Read-path purity (C2)** is the single highest-value correctness fix — closes replica divergence *and* fencing-reuse at once; it's the only path minting authority off the log.
2. **C3+H1+M12 chain**: a clock jump or partition → all-Dead-but-known nodes → empty-replica wipe. The C3 floor guard alone neutralizes the worst outcome (effort S).
3. **`generation` (H11) and fencing tokens (C2/C4)** are advertised as fences but are per-process counters — tie them to a real monotonic source (Raft commit index / persisted high-water).
4. **Persistence rigor**: node's `persist.rs` is reference-quality; the brain's Raft store (M5/M7/M9/M10) and the bridge/lambda "durable" stores (C4/H5) have not adopted the same reject-on-corruption / fail-closed discipline. Porting it is a coherent workstream.
5. **The generated `fiducia-interfaces` crate is a test-oracle only** — production code hand-rolls wire types (H15/M24/M25), which is *why* contract drift accumulates unseen.
