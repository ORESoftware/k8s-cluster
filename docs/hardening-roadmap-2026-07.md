# Hardening roadmap — next steps (2026-07-18)

How to shore up the platform, in order, turning
[`discovery-backlog-2026-07.md`](./discovery-backlog-2026-07.md) (the *what*) into a
*how* and a *sequence*. Item IDs (C#, H#, M#, P3-#) refer to that backlog. Pairs with
[`architectural-security-findings.md`](./architectural-security-findings.md) and
[`SECURITY-AUDIT.md`](./SECURITY-AUDIT.md).

**Status (completed and pushed):** C1 customer@d755ecf; C3 brain@caf03ae; M6
brain@1742894 + node@7ce5ce3; H17 infra@615c2f5; H12 auth@681e27d;
H13/H14 auth@681e27d + customer@f049634; C2 node@c59fa24; C4 bridge@1ff293c.
Items not marked **Done** below remain open.

---

## Guiding principles (apply to every fix)

1. **Fences must be backed by a real monotonic source, never a per-process counter.**
   Fencing tokens (C2, C4) and brain placement `generation` (H11) are all advertised as
   monotonic but reset on restart or mint off-log. Tie each to the Raft commit index or a
   persisted high-water that can only advance.
2. **Read paths are pure.** No read handler may mutate replicated state or mint authority
   (C2). Reads compute "live at now" without promoting/expiring in place; all mutation
   happens in `apply_at` under the log's `proposed_at_ms`.
3. **Fail closed, and prove it with a test.** Every "unavailable → 503 / reject" branch
   needs a negative test (see the P3 gaps). A fail-closed path with no test silently
   becomes fail-open on the next refactor.
4. **Port node's persistence rigor to brain.** `fiducia-node.rs/src/persist.rs` is
   reference-quality (torn-vs-corrupt distinction, fail-closed load). The brain's
   `raft_store.rs` and follower-splice path (M5/M7/M9/M10/M11) have not adopted it. Treat
   this as one coherent workstream, not scattered fixes.
5. **Migrate with dual-accept, never a big-bang cutover.** For anything touching the wire
   or the deploy contract (identity split, `aud` claim, lease ownership), add the new
   behavior as *additive + accepted-alongside-legacy* first, flip enforcement per-env with
   a boolean, remove legacy last. Any phase must be independently shippable and reversible.
6. **Test-first for Raft/coordination.** C2/C3 and the brain-Raft cluster change safety-
   critical invariants. Write the failing test (P3-1..5, 8, 9) *before* the fix so the fix
   is provably correct and guarded forever.

---

## Sequenced workstreams

### WS-0 — Quick wins (small, low-risk, high-value; do first)
Ship these independently; each is S-effort and clearly correct.

| Item | Fix | Repo |
|------|-----|------|
| **C3 — Done** | Floor guard: if `healthy_ids.len() < rf`, treat the tick as incomplete membership — never propose a replica set smaller than `min(current.len(), rf)`. Prevents fleet-wide empty-placement wipe. | brain `scheduler.rs:181` |
| **H10** | Set `FIDUCIA_BRAIN_ID`/`FIDUCIA_NODE_ID` from `topology.toml`'s dialable `*_endpoint` (host:port), not `$(POD_NAME).$(CLUSTER)`. | infra `base/components/brain/statefulset.yaml:67`, `base/node/statefulset.yaml:97` |
| **H17 — Done** | Add a `maxUnavailable:0` PodDisruptionBudget for the brain StatefulSet (mirror node/LB). | infra `base/components/brain/pdb.yaml` (new) |
| **H12 — Done** | Add `aud:"fiducia-api"` to auth's `Claims`/`mint_with` (confirm which token flows through edge/LB first). | auth `token.rs:36` |
| **M6 — Done** | Dedup + self-exclude `peers` at config parse in both node and brain; derive quorum/commit-count from the deduped set. | node `consensus.rs:296`, brain `raft.rs:323` |
| **M21/M22** | Resolve core images to `@sha256:` digests in deploy.yml; add `x-fiducia-*` header keys to the otel redact processor. | monorepo `deploy.yml:110`, infra `otel-agent.yaml:120` |
| **M26/M27** | Delete the two dead auth JWT flags (or wire them); add `FIDUCIA_KV_ENCRYPTION_KEY`, `CUSTOMER_API_KEY_PEPPER`, `FIDUCIA_ALLOW_INSECURE_INTERNAL` to the respective `[env].ignore`. | auth/node/memory `.cli-flags.toml` |

**Definition of done for WS-0:** each fix + a test (where code) or a `kubeconform`/`terraform fmt` pass (infra); committed per-repo; C3 additionally gets the P3-? reconcile test below.

### WS-1 — Coordination-core correctness (the big rock)
Test-first. This is the crown jewel; a wrong fix is worse than the bug.

- **C2 (CRITICAL, Done)** — Make `semaphore_inventory`/`election_inventory`/`*_get` expiry-*aware
  but pure (compute live-at-now, no `expire_due`, no promote, no `next_token`), mirroring
  `record.view(name, now)` already used by `barrier_get`/`task_get`. Move any promotion/
  minting exclusively into `apply_at`. **First write P3-2** (fencing monotonicity across
  snapshot→restore) and a "read on a follower does not change applied state or mint a token"
  test, then fix. node `state.rs:2291,2301,2409,3354`.
- **Brain-Raft rigor cluster (M5, M7, M8, M9, M10, M11, M12)** — one workstream porting
  node's discipline: checksum on persisted files + inverse base_index check (M5); reject
  gapped/at-or-below-base AppendEntries (M7); derive `match_index` leader-side (M8);
  hard runtime truncation guard + re-clamp `commit_index` (M9); torn-vs-corrupt log record
  distinction (M10); `ForgetNode` after grace (M12). Add P3-4 (AssignShard through Raft
  commit/snapshot) and P3-5 (reconcile fixed-point).
- **H11** — derive brain placement `generation` from the Raft commit index; confirm the
  node/LB poller compares `!=` not `>`.
- **C4 (Done)** — persist the compatibility file-lease fencing high-water and active leases
  in a synchronous local journal; restore unexpired leases and a non-regressing token floor
  on boot. The Postgres mirror is best-effort, so it is not a fencing authority. bridge
  `state.rs:121,152,153`.

### WS-2 — Failure-detection & placement safety (chains with C3)
- **H1** — track brain node liveness on `Instant`/monotonic elapsed, not `SystemTime`;
  keep wall-clock only for the human-readable `last_seen_ms`. Neutralizes the NTP-jump →
  all-Dead → C3 chain. brain `scheduler.rs:334`.
- **M13** — when trimming for scale-down, preserve ≥RF distinct failure domains (or feed
  `plan_replicas` the full healthy set, using `target_nodes` as evacuation bias only).
- **M23** — make the brain read `FIDUCIA_RAFT_*` + `FIDUCIA_REPLICATION_FACTOR` (it *is* the
  cross-cloud Raft group), or drop them from its env surface and document node-only.

### WS-3 — Messaging/sync delivery integrity
- **H3** (S-M, do early) — at relay startup assert `stream.duplicate_window >=
  min_duplicate_window(claim_ttl)`, fail closed, and document it as a hard broker
  precondition. messaging `outbox.rs`/`main.rs`.
- **H4** — wire `ensure_consumable(now, require_fencing=true)` into every mutating consumer
  (lambda, agent-manager); add a crate consume-helper that won't hand a payload to a handler
  until the gate passes.
- **H2** — add a delete tombstone (deleted-id→version) consulted in sync `reconcile` and the
  SDK `_applyChange`; add the `Delete v6`→`Upsert v5` convergence test.
- **M1–M4** — compat-relay tx-across-IO + dead-letter (M1/M2), deprecate the pool
  `inbox_try_insert` toward `PgInbox` (M3), split scheduler claim-from-complete (M4).

### WS-4 — Auth assurance-level (root-cause hardening behind the C1 fix)
- **H13 (Done)** — thread the verified factor state / token `aal` into `UserCtx`; have `/v1/me` (or
  the customer session context) reject or downgrade an aal1 session when the account has a
  verified factor. This is the *token-level* backstop; C1 fixed only the one flow.
- **H14 (Done)** — require a fresh aal2 (current TOTP) before any factor mutation (enroll/disable),
  so a password-only session can't strip MFA.
- Backfill P3-6 (real-signature JWKS coverage), P3-7 (CAS exhaustion), P3-11 (aal1 rejection),
  P3-12 (alg-confusion regression guards).

### WS-5 — Infra reliability & supply chain
- **H16** — wire a real remote Terraform backend (S3+DynamoDB or GCS, `encrypt=true`),
  `init -migrate-state`; never commit state.
- **M18** — restrict the kubelet-probe NetworkPolicy to the node CIDR (per-overlay `ipBlock`)
  or drop :8091 from it; optionally add the trusted-hop guard to the sidecar `/metrics`+`/meta`.
- **M19** — add a vultr firewall/allowlist with a world-open precondition, mirroring civo.
- **M20** — minimal SLO PrometheusRules for the coordination core (brain leader present,
  ≥2/3 brain members up, node heartbeat freshness, shard-without-leader).

### WS-6 — Contract drift + per-actor identity + test backfill
- **Contract drift** (H15, M24, M25, M29) — the root cause is that `fiducia-interfaces` is a
  *test-oracle only* while production hand-rolls wire types. Highest-leverage fix: add a CI
  conformance test diffing `operations.json` + the interfaces schema against what each
  service actually serves/parses, so drift fails CI instead of shipping. Then resolve each
  concrete mismatch (semaphore `max`/`limit`, policy optionality, memory claims contract,
  the `/v1/rw/*` 404).
- **Per-actor identity** — execute the phased design in the backlog (Phase 0 dual-accept
  extractor → Phase 1 operator/service split → Phase 2 lease-owner binding + stop leaking
  `fencing_token` (fixes H8) → Phase 3 signed tokens + lambda tenant scope → Phase 4 retire
  shared secret). Each phase ships alone.
- **Test-coverage backfill (P3-1..12)** — land these as guardrails alongside the fixes they
  protect; several (P3-1, P3-3, P3-8) are S-effort and catch catastrophic regressions.

---

## Per-P0 fix recipes

### C3 — brain empty-shard-wipe (do this first; ~S)
1. Test: in `scheduler.rs` tests, membership with all-Dead-but-known nodes; run one
   reconcile tick; assert **no** `AssignShard{replicas:[]}` is proposed and `generation()`
   is unchanged.
2. Fix: at the top of the per-shard loop, compute `healthy = membership.healthy_ids()`; if
   `healthy.len() < rf` (or `== 0`), skip placement changes for this tick (log once) — never
   emit a replica set smaller than `min(current.len(), rf)`.
3. Verify: `rustup run 1.95.0-aarch64-apple-darwin cargo test` (with `RUSTC`+`RUSTDOC`
   pinned — see below).

### C2 — node read-path mutation (Done; test-first)
1. Tests: (a) "inventory read on a follower leaves `last_applied` and the fencing counter
   unchanged"; (b) P3-2 monotonic-mint-after-restore.
2. Fix: introduce pure `*_live_at(now)` variants for lock/semaphore/election inventories that
   compute expiry/promotion *views* without mutating `self`; route `handle_query_local`
   through them. Leave `expire_due`/`*_promote`/`next_token` reachable **only** from `apply_at`.
3. Verify: the full node suite (209 tests) stays green; the new purity and
   snapshot→restore monotonicity tests pass.

### C4 — bridge lease durability (Done)
1. Test: acquire a lease (token N), simulate restart (rebuild `State`), acquire again; assert
   the new token `> N` and the prior lease is either restored or its token never reused.
2. Fix: synchronously journal the compatibility lease fencing high-water and active leases;
   fsync the journal before responding and restore a non-regressing token floor (and
   unexpired leases) on boot. The asynchronous Postgres mirror is not used as a fence.
3. Verify: the bridge unit, hardening, HTTP, preflight, and TCP test suites pass.

---

## Verification recipe (so fixes are provably green locally)

- **Toolchain shadow:** a Homebrew `rustc`/`rustdoc` 1.96.1 on `PATH` shadows rustup and
  breaks doctests with spurious `E0514`. For every Rust repo, pin both:
  ```sh
  TC=<toolchain>   # 1.95.0-… for node/brain/LB; 1.97.0-… for customer; stable-… otherwise
  export RUSTC="$(rustup which --toolchain $TC rustc)"
  export RUSTDOC="$(rustup which --toolchain $TC rustdoc)"
  CARGO_BUILD_JOBS=2 rustup run $TC cargo test
  ```
  **Worth doing:** bake this into each repo's CI/test script so it doesn't bite again.
- **Disk:** the workspace's cargo `target/` dirs total ~50 GB; `cargo clean` finished repos
  between cold builds if free space drops below ~5 GiB.
- **Browser E2E:** the customer MFA/OTP + separation journeys live in `fiducia-e2e`
  (`npm run test:browser`, heavy — boots 3 servers + Postgres). Extend them with a
  **password-login-forces-step-up** browser case to complement C1's unit regression.

---

## Suggested order of attack

1. **WS-0 remaining quick wins** — H10, config/supply-chain hygiene.
2. **WS-1 remaining coordination core** — brain-Raft rigor, H11, test-first.
3. **WS-2/WS-3** — failure-detection safety and messaging integrity.
4. **WS-5 infra** and **WS-6 contract/identity/tests** — steady background workstreams.

Each item in the backlog carries CONFIRMED vs SUSPECTED — start CONFIRMED, and reproduce any
SUSPECTED item (H5 brain drain-ack, M-signedness, poller comparison operator) before changing
code.
