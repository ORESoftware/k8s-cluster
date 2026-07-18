# Use-Case Exploration

Speculative fit analysis for four proposed directions. Not a roadmap — a
capability/limitation map so we don't oversell fiducia into problems it can't
honestly solve. Each idea is scored and reduced to its owning primitives.

## The two questions that decide every fit

Fiducia is **consensus & coordination as a service**: a sharded multi-Raft data
plane (`fiducia-node.rs`) exposing multi-key locks, semaphores, RW-locks,
idempotency keys, rate limiting, cron, linearizable KV+watches, leader election,
and service discovery. Its identity in one line:

> **exactly-once + single-authoritative-owner + fencing, at low latency and high
> availability across clouds, within a trust boundary.**

Two properties gate whether any use case fits. Ask them first, every time:

1. **Fault model — crash (CFT) or adversarial (BFT)?** Raft assumes our own
   non-malicious nodes may *crash*; it assumes they never *lie*. One operator
   owns the quorum. Anything needing mutually-distrusting participants (public
   blockchains, public elections) needs a BFT + cryptographic layer fiducia does
   not provide.
2. **Data — operator-visible OK, or must it be secret?** The Raft log/KV is
   replicated plaintext to RF=3 across AWS/GCP/Hetzner. Secrets (OTP seeds,
   secret ballots) require client-side crypto *on top*; fiducia never becomes the
   secret store itself.

Fit ✅ when the answer is (crash-fault, operator-visible). Everything else needs
an explicit second layer, and we say so.

---

## Idea 1 — Backend for 2FA / 3FA — ★★★☆☆ (partial)

**Not the vault; yes the verifier fleet.** The `3FA-app` is deliberately
zero-knowledge — the sync server never sees OTP seeds; vaults are E2E-encrypted
client-side (XChaCha20-Poly1305 + Argon2id). That rules out the naive version:

- ❌ Fiducia is **not** the secret vault — its KV is plaintext RF=3. Moving seeds
  there breaks zero-knowledge. The existing version-vector sync is also lighter
  than a Raft lock on the sync path.

But a 2FA/3FA system is a **distributed verification fleet**, and that is pure
coordination:

- ✅ **OTP single-use / replay prevention** — idempotency key on
  `(account, code, time-step)` + TTL → a TOTP code verifies **exactly once across
  the fleet**, not once-per-node. Closes the "same 30s code accepted twice" hole.
- ✅ **Brute-force protection** — distributed token-bucket per account+IP;
  consistent where per-node counters are bypassed by spraying replicas.
- ✅ **Push-based 3FA challenge store** — the "approve on phone" nonce must be
  consumed exactly once before expiry: idempotency/KV-with-TTL + fencing token.
- ✅ **Instant device revocation** — KV watch on a revocation set kills every
  verifier in ms instead of waiting out a cache TTL.
- ✅ **Step-up / session elevation** — one authoritative lease record.

Cheap POC: make `dd-remote-auth`'s ±1-step TOTP window single-use across
AWS+Hetzner via a fiducia idempotency key. Small, sharp, demoable.

Owner repos: `fiducia-node.rs` (idempotency, rate-limit, KV+watch), `fiducia-clients`.

---

## Idea 2 — Coordinating AI agents — ★★★★★ (strongest fit; dogfood)

The stated problem — *"agents must not clobber the same branch or the same
files"* — **is the multi-key lock**, our flagship, verbatim. We already live the
pain: `agents.md` enforces worktrees + a hand-maintained command blacklist, which
is a *manual* version of what fiducia can enforce programmatically.

This **complements ai-agent-bridge, does not replace it** — two layers:

- **ai-agent-bridge** = the *conversation* bus (topic rooms, SSE, who's talking).
- **fiducia** = the *arbitration* layer (who **owns** what, exactly-once).

Chat says what's happening; fiducia decides who may act. Mappings, all needed today:

- ✅ **File/branch clobber → multi-key union lock** over the paths in the intended
  diff (or `branch:main`). Atomic all-or-nothing, FIFO, deadlock-free.
- ✅ **Crashed/hung agent → TTL leases.** LLM agents stall and die constantly; a
  dead agent's lock auto-expires instead of wedging the repo. Justifies it alone.
- ✅ **Stale agent that returns → fencing tokens.** An expired-lease agent's commit
  is rejected by a pre-commit hook checking the token. Kills the zombie-overwrite.
- ✅ **One integrator per branch → leader election.**
- ✅ **Retry / non-determinism → idempotency keys** (no duplicate PR / double migration).
- ✅ **Shared model API or CI runner caps → semaphores.**
- ✅ **Shared task board → KV + watches.** Agent registry → service discovery.

First build: a git pre-edit/pre-commit hook (or thin `agent-coord` wrapper) that
calls `lockMany({keys: <diff paths>, holder: <agent-id>, ttlMs})` before editing
and threads the fencing token into the commit. Few dozen lines on the existing
TS/Rust client, solves a problem we hit daily, and makes fiducia self-hosting on
its own tooling.

#### Embedding-routed topic rooms (where the two layers join)

Agents don't know channel names up front — they know a *topic* ("refactor the
auth module", "fix the flaky payment tests"). Routing is by **embedding
similarity**: embed the topic, look it up against existing rooms, join the
nearest (or mint a new one). This is already ai-agent-bridge's design — channels
resolved by cosine similarity, capped at 32 members — and the persistence is a
**Postgres pgvector** lookup: `embedding → nearest channel row → topic/slug`
(the `ai_agent_bridge` schema, `channels` table, when built `--features
postgres`).

Keep the layer split clean:

- **Routing / topic discovery = ai-agent-bridge** (pgvector: embedding → room).
  This is *communication* — it decides who's in the conversation.
- **Arbitration = fiducia** — it decides who may *act*. Fiducia does **not** do
  the vector search; that stays in Postgres/ai-agent-bridge.

The join point is that **an embedding-identified topic room is the natural
coordination scope**. Once agents converge on a room, that room's slug namespaces
their fiducia state:

- **Room → lock namespace.** Agents in `topic:refactor-auth` take multi-key locks
  under that prefix over the files that topic touches — coordination is scoped to
  the conversation that motivated it.
- **Room → shared blackboard.** The room's task/claim board is fiducia KV keyed by
  room slug, watched by every member (durable + linearizable, where the chat
  stream is ephemeral).
- **Room → presence + integrator.** Service discovery is per-room agent presence;
  leader election picks the room's one integrator/merger.

So the pipeline is: *topic → embedding → (pgvector) room → (fiducia) locks + KV +
election scoped to that room*. ai-agent-bridge owns the left half, fiducia the
right; the room slug is the shared key between them. Neither reimplements the
other — no vector search in fiducia, no Raft in the bridge.

Owner repos: `fiducia-node.rs`, `fiducia-clients`, `fiducia-cli.rs`; interop with
`ai-agent-bridge.rs` (pgvector routing, `ai_agent_bridge` schema).

---

## Idea 3 — Validating financial / crypto transactions — ★★★★☆ (within a trust boundary)

**Raft is CFT, not BFT.** Blockchains exist precisely because validators may be
adversarial and mutually distrusting. Our nodes are one operator's machines
assumed never to lie.

- ❌ Fiducia **cannot** be a trustless/permissionless validator — no economic or
  Byzantine security. Never claim otherwise.

But CFT consensus has a large, *legitimate* role beside/under the ledger:

- ✅ **Exactly-once payment execution** — "don't double-send payouts/invoices" is
  already in the node README. Idempotency keys are the canonical defense.
- ✅ **Ordering service for a *permissioned* ledger** — the honest strong angle: a
  linearizable log **is** a total-order sequencer. Hyperledger Fabric's ordering
  service is Raft; fiducia can play that exact role for a consortium ledger.
- ✅ **Custody double-spend prevention (custodian's own side)** — multi-key lock
  over `{hot_wallet, account}` + fencing token into the ledger row serializes
  withdrawals. Leader election picks the single settlement writer. How exchanges
  use etcd/Zookeeper today.
- ✅ **Threshold-signing / MPC ceremony coordination** — the signing is crypto
  elsewhere, but coordinating an m-of-n round and **guaranteeing no ECDSA/Schnorr
  nonce reuse** (catastrophic key leak) is exactly a lock/idempotency guard.
- ✅ **Bridge/oracle/relayer dedup** — idempotency key on the message hash.
- ✅ **Transaction velocity limits** — rate limiter per account.

Framing to lead with: **coordination + total-ordering layer for permissioned
finance, not a chain.** Trustless validation would need a *different engine*
(Tendermint/HotStuff/BFT-Raft + signed verifiable logs) — a separate research
track, not a Raft config flag.

Owner repos: `fiducia-node.rs`, `fiducia-clients`, `fiducia-interfaces`.

---

## Idea 4 — Other coordination use cases + hosting elections

### Generic coordination catalog — ★★★★☆ (cheap, sellable)

Natural endpoint-level extensions, not new engines: distributed config /
feature-flag rollout (KV+watch), distributed cron without double-firing,
saga/workflow coordinator, 2PC/distributed-transaction coordinator, distributed
barrier / countdown latch, FIFO work queue on the Raft log, monotonic
sequencer/ID generator (we already mint fencing tokens — expose as a pure
sequence service), license/quota semaphores, blue-green/canary gates,
schema-migration singleton gate, m-of-n approval chains (counting semaphore).

### Elections — ★★★☆☆ (governance yes; public political elections need a crypto layer)

Terminology trap: Raft "leader election" elects a *server coordinator* — nothing
to do with ballots. The real question is human/organizational voting.

What fiducia's guarantees give a voting system:

- ✅ Tamper-evident, totally-ordered, replicated **ballot log** (durable, multi-cloud).
- ✅ **One-person-one-vote** — idempotency key per eligible voter token.
- ✅ Eligibility/roll coordination, quorum thresholds, delegation graphs (liquid
  democracy in KV), quadratic-voting tallies.

What it fundamentally **cannot** provide (do not oversell — election integrity is
a minefield):

- ❌ **Ballot secrecy / anonymity** — log is operator-visible; real voting needs
  homomorphic encryption / mixnets / blind signatures.
- ❌ **Distributed trust** — public elections need mutually-distrusting authorities
  + threshold decryption; fiducia is one operator's CFT quorum.
- ❌ **Coercion resistance, end-to-end + universal verifiability** — the hard parts
  of e-voting; fiducia addresses none directly.

Honest posture: fiducia is the **ordered, replicated, exactly-once ballot ledger
+ eligibility/quorum coordinator** — the "database" underneath — while the
cryptographic protocol (Helios/Belenios/ElectionGuard-style) and BFT trust
distribution are a **separate layer on top**. Good enough today *without* the
heavy crypto for **lower-stakes governance**: DAO/board/shareholder/union votes,
cooperative governance. One elegant niche: **sortition** (random jury/citizens'-
assembly selection) needs a *verifiable public-randomness beacon* — a
commit-reveal beacon anchored in fiducia's ordered log is a clean fit. Public
political elections: position fiducia as infrastructure a certified voting
protocol *runs on*, never as the integrity guarantee itself.

Owner repos: `fiducia-node.rs`, `fiducia-clients`, `fiducia-interfaces`.

---

## Broader use-case surface

Everything here is native (crash-fault, operator-visible) territory — fiducia's
sweet spot, no second layer required. This is the catalog a customer maps their
problem onto. Grouped by the primitive that powers it.

### By primitive

**Multi-key locks** — single-writer serialization:
- Terraform / infra **state locking** (the DynamoDB-lock-table role).
- **Deploy locks** — only one deploy to an environment at a time.
- **Migration guard** — exactly one schema migration runs.
- Single-writer to a non-transactional external system (an S3 object, a legacy
  API, a file) — serialize writes fiducia can't see.
- **Resource checkout** — a staging slot, a test device, a physical fixture.
- Tenant / shard / partition **ownership** (one worker owns one key).

**Semaphores** — bounded concurrency:
- Cap workers hitting a **rate-limited third-party API**.
- **License / seat** enforcement (N concurrent users of a paid feature).
- **GPU / job pool** — only N training or batch jobs at once.
- Shared **connection-pool / quota** ceiling across a fleet.
- Canary cohort sizing.

**RW-locks** — many readers / exclusive writer:
- Config reload; **cache-stampede** prevention (one loader repopulates, readers wait);
- Block writers during a **consistent backup / snapshot**.

**Idempotency keys** — exactly-once:
- **Webhook / event** dedup (Stripe events, at-least-once queues, Kafka/SQS consumers).
- Retry-safe payments / order placement.
- **Notification dedup** — don't send the same email/SMS/push twice.
- Outbox-pattern dedup.

**Rate limiting** — per-tenant fairness & cost control:
- Per-tenant **API quotas** (consistent across replicas).
- Login/OTP **brute-force throttle**; abuse/spam control.
- **LLM spend caps** per tenant; fair-share scheduling; ad **budget pacing**.

**Cron / schedules** — no double-fire across a fleet:
- Distributed scheduled jobs, delayed jobs / reminders.
- Billing runs, report generation, retention cleanup.
- **Certificate renewal / token rotation** triggers; watchdog heartbeats.

**KV + watches** — live shared state:
- Dynamic **config / feature flags** with push (no poll).
- **Kill switches / circuit-breaker** state shared fleet-wide.
- Dynamic **routing tables / traffic splitting**, runtime log-level control.
- Cache-invalidation signal; A/B experiment assignment; shared blackboard.

**Leader election** — active/standby singletons:
- Failover for a scheduler / poller / reconciler; **regional primary** selection.
- **Kubernetes-operator / controller** leader election (etcd's own headline use).
- Coordinator election inside a customer's own app.

**Service discovery** — live membership:
- Dynamic endpoint registry replacing stale DNS; health-aware routing;
- Blue-green / canary member sets; sidecar-mesh membership; locality-aware lookup.

### Composed primitives

Several of these have since shipped as first-class node primitives
(fiducia-node.rs `main.rs` mounts them under `/v1`): **fan-in barriers** incl.
quorum/weighted/veto policies (`/v1/barriers` — covers the countdown-latch
shape), a **claimable FIFO work queue** (`/v1/tasks` over the indexed queue),
**counter CAS** (`/v1/counters` with `prev_revision` — single-key CAS), and an
**approval-escrow effect** flow (`/v1/effects` — the 2PC-adjacent
prepare/approve/commit shape).

Still roadmap, not available today: **gang scheduling** on top of barriers, a
**sequencer / monotonic ID** service, a general **saga / workflow** coordinator,
true **cross-shard 2PC / multi-key CAS** (same-shard only today), and
**fencing-token-as-a-service** for external stateful systems (DB rows, S3,
payment idempotency tables).

### By vertical (the buyer-facing framing)

| Vertical | Fiducia earns its place doing |
|----------|-------------------------------|
| **CI/CD** | deploy locks, environment reservation, concurrency groups, release-train ordering |
| **Data pipelines** | exactly-once batch, Kafka-consumer partition ownership, watermark coordination |
| **Multi-region active-active** | write ownership, conflict avoidance, regional failover |
| **Kubernetes / operators** | controller leader election, dynamic operator config |
| **IoT / edge fleets** | device-shadow config, command dedup, firmware-rollout gating |
| **Gaming / realtime** | matchmaking authority, room/session ownership, seat reservation |
| **E-commerce** | inventory reservation (oversell prevention), checkout lock, flash-sale rate limiting |
| **Ad tech** | budget pacing, frequency capping |
| **Databases / storage** | migration gate, backup coordination, primary election for a customer DB |
| **Batch / HPC** | gang scheduling, GPU-pool semaphores |

The pattern behind all of them is the same three-word test: **exactly-once**,
**single-owner**, **fenced** — whenever a customer has many replicas but a
decision that must have one authoritative outcome, that's a fiducia call.

---

## Summary

| Idea | Fit | One-line reason |
|------|-----|-----------------|
| 2 — AI agent coordination | ★★★★★ | Multi-key lock = the clobber problem verbatim; TTL+fencing handle dying agents; we're the user today. |
| 3 — Permissioned finance / ordering | ★★★★☆ | Exactly-once payouts, custody serialization, Fabric-style ordering — all CFT-appropriate. Never "trustless". |
| 4 — Generic coordination catalog | ★★★★☆ | Barriers, queues, sequencers, saga/2PC — each a cheap endpoint, each sellable. |
| 1 — 2FA/3FA fleet | ★★★☆☆ | Not the vault; yes the verifier fleet — OTP single-use, brute-force limits, revocation propagation. |
| 4 — Governance / low-stakes elections | ★★★☆☆ | Great as ballot ledger + quorum coordinator; public elections need a crypto/BFT layer on top. |

Through-line: ideas 2 and the ordering half of 3 fit fiducia's identity with zero
stretching. Ideas 1, 4-elections, and trustless-3 all require an explicit second
layer (client-side crypto for secrecy, or BFT for adversarial trust) — worth
pursuing, but only while being honest that fiducia is the coordination substrate
underneath, not the security guarantee itself.
