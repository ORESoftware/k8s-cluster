# Future Work

## Storage Decision

Config KV and the other coordination primitives are not backed by
Postgres/Supabase. Their source of truth is the owning shard's Raft log plus the
applied state-machine snapshot on each `fiducia-node`.

Use embedded RocksDB per node for production durability:

- `fiducia-node.rs`: Raft log, Raft metadata, snapshots, applied coordination
  state, and watch indexes under `FIDUCIA_NODE_DATA_DIR`.
- `fiducia-interfaces`: Supabase/Postgres schema only for business-plane data:
  orgs, projects, users, RBAC, API keys, mTLS identities, audit, billing, and
  dashboard metadata.
- `fiducia-brain.rs`: brain membership and shard-placement state eventually
  needs its own small replicated brain Raft group; it should not use the
  customer coordination KV as its control-plane database.

## Highest-Value Product Gaps

1. **Auth, RBAC, and audit**
   Owner repos: `fiducia-auth.rs`, `fiducia-interfaces`, `fiducia-admin.rs`,
   `fiducia-edge`.

   Add orgs, projects, per-key permissions, scoped short-lived tokens,
   project-scoped API keys, audit logs, and optional mTLS client certificates.
   API keys stay the customer DX; auth/LB caches introspection so the hot path
   does not call Supabase or Postgres on every request.

2. **SDKs and CLI**
   Owner repos: `fiducia-clients`, `fiducia-cli.rs`, `fiducia-interfaces`.

   Treat TypeScript, Go, Rust, and Python as the first production tier. Generate
   payload types from `fiducia-interfaces`; keep `fiducia-clients/PROTOCOL.md`
   as the method/endpoint contract. Grow `fiducia-cli.rs` from closest-region
   selection into `fiduciactl`: login, project selection, API key lifecycle, KV
   get/put/watch, lock/semaphore/multi-key inspect, schedule history, shard
   health, and support bundles.

3. **Admin and observability APIs**
   Owner repos: `fiducia-node.rs`, `fiducia-brain.rs`, `fiducia-admin.rs`,
   `fiducia-telemetry.rs`, `fiducia-backend.rs`.

   Expose lock holders, FIFO wait queues, lease state, leader history, schedule
   run history, shard health, quorum status, per-shard latency, Raft term/index,
   snapshot/compaction lag, and route/redirect counts. Admin UI should be a thin
   authenticated web tier over those APIs.

4. **Transactions and workflows**
   Owner repos: `fiducia-node.rs`, `fiducia-interfaces`, `fiducia-clients`.

   Multi-key locks and capped semaphores are now part of the node/client
   contract. Next, add atomic multi-key CAS when keys share a shard, composite
   wait/idempotency semantics, lock plus config update, semaphore plus fencing
   token in one workflow, and a cross-shard workflow story for operations that
   cannot be made single-shard.

5. **Disaster recovery and retention**
   Owner repos: `fiducia-node.rs`, `fiducia-brain.rs`, `fiducia-infra`,
   `fiducia-admin.rs`.

   Add snapshot export/import, restore drills, regional failover, schedule run
   retention, audit retention, watch-history retention, and fencing-token-safe
   restore semantics.

6. **Service discovery depth**
   Owner repos: `fiducia-node.rs`, `fiducia-load-balance.rs`,
   `fiducia-routing.rs`, `fiducia-clients`.

   Add DNS endpoint support, active health checks beyond TTL heartbeat, tags,
   metadata filtering, prepared queries, locality-aware lookup, and load-balancer
   integration.

7. **Compatibility layer**
   Owner repos: new adapter repo or `fiducia-edge`, plus `fiducia-node.rs`.

   Consider an etcd-compatible subset first: KV get/put/delete/watch, leases,
   locks, and elections. A ZooKeeper-style recipe layer can follow if customers
   need migration without rewriting coordination code.

## Strategic Use Case Tracks

Fiducia's strongest product shape is a linearizable coordination substrate:
not an authenticator app, not a cryptocurrency chain, and not a voting machine
by itself, but the consistent backend that keeps those systems from racing,
double-spending, double-claiming, or drifting into split brain. These tracks are
worth exploring as explicit product surfaces on top of the existing primitives.

### 2FA, 3FA, and security ceremonies

Fiducia can help a 2FA/3FA product, such as the sibling `3FA-app`, as the
coordination and policy backend around the actual factor logic. It should not
store TOTP seeds, decrypt encrypted vaults, or decide that three prompts are
automatically "3FA". The authenticator should keep cryptographic factor checks,
vault encryption, voice/passkey/biometric proof, and zero-knowledge sync local
or app-specific.

The useful Fiducia role is ceremony coordination:

- device enrollment locks: one active enrollment per user/device;
- recovery approvals: 2-of-3 or 3-of-5 trusted device/admin/member approvals;
- step-up grants: short TTL capability records after passkey, biometric, voice,
  or other phishing-resistant proof;
- revocation fanout: KV/watch streams for revoked devices, sessions, API keys,
  and recovery codes;
- sync safety: CAS and idempotency around encrypted vault version updates;
- anti-replay: idempotency records and fencing tokens for login, enrollment,
  recovery, and key-rotation attempts;
- enterprise policy: require distinct factor kinds before dangerous actions,
  such as production API-key issuance or org-owner transfer.

This track fits `fiducia-auth.rs`, `fiducia-interfaces`, `fiducia-admin.rs`,
`fiducia-customer-ui.web`, and `fiducia-node.rs`. The product should emphasize
phishing-resistant factors like passkeys/WebAuthn where possible, while using
Fiducia to make the surrounding lifecycle auditable, race-free, and recoverable.

### AI agent coordination

AI-agent coordination is likely the sharpest near-term Fiducia use case. Agent
fleets need the same primitives as distributed services: claim work once, avoid
clobbering shared branches/files, gate scarce tools, elect one supervisor,
publish state changes, and recover when a worker hangs.

An agent bridge for Codex, Claude, Gemini, local runners, and CI workers could
standardize keys such as:

- `repo:<repo>:branch:<branch>` for branch ownership;
- `repo:<repo>:file:<path>` for edit ownership;
- `task:<id>` for exactly-once task claims;
- `tool:<kind>:<id>` for browser sessions, terminals, sandboxes, GPUs, inboxes,
  and paid APIs;
- `budget:<org>:<provider>:<model>` for spend and quota control;
- `memory:<scope>` for shared context or note mutation;
- `chatroom:<topic-id>` for topic-scoped agent conversation rooms.

The flagship primitive here is the multi-key union lock. An agent can lock the
branch plus the exact file set it intends to edit. Another agent can still work
on disjoint files. TTL leases clean up abandoned claims, FIFO queues make
contention visible, and fencing tokens prevent a stale paused agent from
committing after its lease expired.

This track should become a first-class `agent-coordinator` product layer:

- task claim and result-commit APIs over idempotency records;
- file/branch/tool lock manifests;
- live heartbeats and service discovery for active agents/tools;
- watch streams for handoffs, review state, and supervisor decisions;
- embedding-pinned thread rooms: each agent thread stores one or more vectors,
  Postgres/pgvector maps a new embedding to the nearest chatroom topic, and
  Fiducia coordinates the live room lease, membership, topic metadata, and
  watched message/result pointers;
- Git pre-commit/pre-push hooks that verify lock ownership before writing;
- MCP, CLI, and SDK adapters so agents do not hand-roll lock usage;
- an admin UI showing active agents, leases, queues, held files, stale holders,
  and failed handoffs.

The chatroom layer should keep semantic routing and coordination separate.
Postgres owns durable topic metadata, transcripts, embeddings, nearest-neighbor
lookup, retention policy, and search. Fiducia owns the coordination envelope:
who is currently in the room, which supervisor owns the topic, which agents may
write summaries or decisions, which result pointer is current, and which
watchers should wake up when a room changes. That gives agents "chatrooms by
topic" without making the Raft data plane a vector database.

Fiducia will not solve semantic merge conflicts by magic, but it can prevent
the dumb collisions and make the remaining conflicts explicit.

### Financial transaction coordination

Fiducia can validate and coordinate financial workflows in a permissioned
setting, especially when the goal is exactly-once processing and a single
authoritative operation order. Strong fits include:

- webhook dedupe for payment providers;
- payout scheduler leader election;
- balance reservation and release;
- account, wallet, or ledger-row locks;
- transfer idempotency keys with replayable results;
- transaction status KV watched by customers and workers;
- reconciliation worker elections;
- settlement windows and durable schedule runs;
- downstream fencing tokens so stale leaders cannot write after failover.

The right first product is a transaction coordination and ledger-control plane,
not a public cryptocurrency. Raft is crash-fault tolerant: it protects against
node crashes, restarts, partitions without quorum, and stale leaders in a
trusted cluster. It does not by itself protect against malicious validators,
colluding nodes, or public adversarial consensus. Public crypto, consortium
settlement among mutually distrusting parties, and token issuance need a
separate Byzantine-fault-tolerant track, such as PBFT/HotStuff/Tendermint-style
designs, or integration with an existing chain.

Near-term repo owners are `fiducia-node.rs`, `fiducia-interfaces`,
`fiducia-clients`, `fiducia-load-balance.rs`, and `fiducia-auth.rs`. Before
making money-grade claims, harden WAL/snapshot/restore, Jepsen-style
linearizability tests, idempotency retention, audit export, and fencing-token
enforcement examples.

### Elections, governance, and public trust

Fiducia already exposes leader election; that is not the same as human
political elections. The naming overlap is useful for coordination but dangerous
for product claims.

Strong near-term election/governance uses:

- internal company votes and approvals;
- DAO or association governance where legal expectations are limited;
- board, standards-body, union, club, or committee elections;
- voter-roll snapshot coordination;
- one-vote claim records;
- ballot-box open/close schedules;
- tally-worker leader election;
- transparent event logs and signed result checkpoints;
- public watch streams for status, turnout counters, and audit milestones;
- risk-limiting-audit sample coordination for paper-based elections.

For literal public political elections, Fiducia should be framed first as
election infrastructure: transparency logs, chain-of-custody workflows,
registration coordination, audit sampling, result publication, incident
coordination, and verifiable administrative records. It should not be pitched as
internet ballot casting until there is a formally designed end-to-end verifiable
voting protocol, coercion-resistance analysis, voter-verifiable evidence,
certification strategy, accessibility review, and independent audit path.
Consensus alone does not solve malware on voter devices, ballot secrecy,
coercion, identity proofing, recounts, or public legitimacy.

This track belongs behind careful language in `fiducia-ui.web` and deeper
protocol work in `fiducia-interfaces` plus `fiducia-node.rs`.

### Other coordination and cooperation surfaces

Adjacent coordination systems validate the broad pattern. etcd is commonly used
for configuration, service discovery, distributed work coordination, leader
election, locks, and liveness. ZooKeeper recipes cover locks, queues, barriers,
two-phase commit, and leader election. Kubernetes uses Lease objects for
high-availability component leadership. Consul focuses on service discovery,
health, KV/config, and service mesh. Fiducia should not simply copy those
systems; it should productize their coordination recipes as hosted,
HTTP-native, multi-cloud primitives with strong SDKs and a clear control plane.

Additional tracks worth exploring:

1. **Platform control planes and schedulers**

   Owner repos: `fiducia-node.rs`, `fiducia-brain.rs`, `fiducia-routing.rs`,
   `fiducia-load-balance.rs`, `fiducia-infra`, `fiducia-admin.rs`.

   Use Fiducia as the externalized coordination core for schedulers,
   controllers, and platform operators. A customer running many replicas of a
   controller can elect one active reconciler, coordinate shard/tenant ownership,
   drain nodes safely, and publish placement decisions through watched KV.
   Kubernetes already demonstrates the need for lightweight leader leases in
   HA control-plane components; Fiducia can generalize that pattern for any
   non-Kubernetes fleet or cross-cluster operator.

   Concrete products:

   - controller leader election with forced handoff and fencing tokens;
   - tenant/shard ownership leases for schedulers;
   - safe drain/evacuation workflows for workers and customer cells;
   - watched placement maps for edge and load-balancer routing;
   - topology snapshots and route-debug bundles for support.

2. **Service discovery, health, and dynamic config**

   Owner repos: `fiducia-node.rs`, `fiducia-clients`,
   `fiducia-load-balance.rs`, `fiducia-edge`, `fiducia-admin.rs`.

   Fiducia already has the core of Consul/etcd-style service discovery:
   TTL-backed service registration, metadata filters, KV, and watches. This can
   become a simple hosted registry for services that need live endpoints but do
   not want to operate Consul, ZooKeeper, or etcd.

   Concrete products:

   - service registry with health leases and metadata filters;
   - locality-aware service lookup for region/cell-aware clients;
   - dynamic feature flags and config KV with SSE/watch streams;
   - safe config rollout with CAS, staged gates, and rollback pointers;
   - edge config propagation where Cloudflare Workers watch a compact state
     pointer and the regional LB resolves the authoritative route.

3. **Workflow, cron, and job orchestration**

   Owner repos: `fiducia-node.rs`, `fiducia-clients`, `fiducia-interfaces`,
   `fiducia-admin.rs`.

   Fiducia can be a small durable coordinator for jobs where "exactly one run"
   matters more than heavyweight workflow graphs. It should not replace every
   workflow engine. It should own schedule claims, run history, idempotency,
   worker leadership, and result pointers.

   Concrete products:

   - exactly-once cron firing with durable run records;
   - per-tenant job claims and result commits;
   - dead-worker recovery using TTL leases;
   - retry windows with idempotent completion;
   - one active migration, reconciler, report builder, or export worker per
     customer shard.

4. **API, webhook, and event reliability**

   Owner repos: `fiducia-load-balance.rs`, `fiducia-node.rs`,
   `fiducia-clients`, `fiducia-interfaces`.

   Many production systems fail in boring ways: duplicate webhooks, retried
   POSTs, timeouts after success, workers racing to process the same event, and
   outbox rows being delivered twice. Fiducia's idempotency records and fencing
   tokens are a natural hosted solution.

   Concrete products:

   - customer-facing idempotency keys at the LB boundary;
   - webhook event dedupe and replayable completion records;
   - inbox/outbox delivery locks;
   - exactly-one side effect per logical event;
   - event status KV watched by dashboards and support tools.

5. **Quota, spend, and scarce-resource control**

   Owner repos: `fiducia-node.rs`, `fiducia-edge`,
   `fiducia-load-balance.rs`, `fiducia-admin.rs`, `fiducia-clients`.

   Any global scarce resource can use the same shape: a rate limit, a semaphore,
   and sometimes a budget ledger. This applies to APIs, model calls, GPU time,
   SMS/email sends, browser sessions, deploy lanes, customer seats, or human
   review queues.

   Concrete products:

   - global tenant rate limits at edge and regional LB layers;
   - semaphores for fragile tools and paid APIs;
   - spend guards for AI providers or third-party APIs;
   - back-pressure instead of overage bills and 429 storms;
   - admin-visible current holders, wait queues, and quota burn.

6. **Data pipelines, analytics, and ML operations**

   Owner repos: `fiducia-node.rs`, `fiducia-clients`, `fiducia-interfaces`,
   future ML/agent adapters.

   Data and ML fleets are full of coordination problems: partition ownership,
   checkpoint advancement, one trainer per dataset slice, GPU allocation, model
   promotion, and avoiding two jobs rewriting the same artifact.

   Concrete products:

   - partition leases for stream processors and ETL workers;
   - checkpoint CAS for idempotent progress;
   - dataset, feature-store, and model-artifact locks;
   - GPU/accelerator semaphores;
   - one active trainer/evaluator/promoter per model line;
   - watched model-registry promotion pointers.

7. **Database migrations and schema changes**

   Owner repos: `fiducia-node.rs`, `fiducia-clients`, `fiducia-admin.rs`,
   `fiducia-interfaces`.

   Database migrations often need global or per-tenant exclusion but are still
   run from many deploy replicas. Fiducia can provide migration locks,
   migration progress KV, and rollback-safe fencing.

   Concrete products:

   - one migrator per database, tenant, region, or shard;
   - migration phase records watched by deploy tools;
   - schema-version CAS before rollout;
   - expand/contract deployment gates;
   - emergency pause/resume switches for migrations.

8. **Marketplaces, booking, inventory, and reservations**

   Owner repos: `fiducia-node.rs`, `fiducia-clients`, `fiducia-interfaces`.

   This is the same family as financial coordination but broader. Tickets,
   appointments, hotel rooms, inventory units, ad slots, delivery windows, and
   warehouse picks all need short-lived holds, dedupe, and exactly-one
   settlement.

   Concrete products:

   - reservation holds with TTL;
   - multi-key holds across user, item, inventory, and payment-intent keys;
   - fencing tokens passed into the authoritative inventory or booking system;
   - idempotent checkout completion;
   - watched reservation status for customer support.

9. **Supply chain, provenance, and transparency logs**

   Owner repos: `fiducia-node.rs`, `fiducia-interfaces`, `fiducia-admin.rs`,
   possible new transparency-log repo.

   Fiducia can coordinate append-only attestations, signed checkpoints, and
   release gates. It should not claim to be a full transparency log until the
   cryptographic append-only proof model is explicit, but it can coordinate the
   workflow around one.

   Concrete products:

   - release approval quorums;
   - artifact signing ceremony locks;
   - build provenance pointers in KV;
   - one active promoter per package/channel;
   - signed checkpoint publication and watcher notification;
   - audit export for regulators or customers.

10. **IoT, robotics, and edge device fleets**

   Owner repos: `fiducia-edge`, `fiducia-load-balance.rs`, `fiducia-node.rs`,
   `fiducia-clients`, `fiducia-auth.rs`.

   Device fleets need command dedupe, leader gateways, live membership, and
   safe handoff when a controller disappears. Fiducia's HTTP API and leases fit
   command-and-control layers that cannot tolerate duplicate destructive
   commands.

   Concrete products:

   - device/gateway service registration with TTL;
   - one active controller for a robot, site, vehicle, or sensor group;
   - idempotent command IDs with replayable results;
   - maintenance-window locks;
   - watched fleet config and staged rollout gates.

11. **Collaborative applications and game/session backends**

   Owner repos: `fiducia-node.rs`, `fiducia-clients`, future app adapters.

   Collaborative products often need a lightweight authority: who owns the room,
   who may commit the next state, what document version is current, and whether
   a session result already settled.

   Concrete products:

   - document/section ownership locks for collaborative editors;
   - room/session leader election;
   - turn or match settlement idempotency;
   - lobby membership with TTL;
   - watched room state pointers;
   - one authoritative host for a multiplayer room with failover.

12. **Incident response, legal, healthcare, and regulated workflows**

   Owner repos: `fiducia-admin.rs`, `fiducia-auth.rs`,
   `fiducia-interfaces`, `fiducia-node.rs`, `fiducia-customer-ui.web`.

   Regulated workflows often need auditable handoffs rather than raw storage.
   Fiducia can coordinate who is allowed to act, whose approval is still
   missing, and which version of a record was approved.

   Concrete products:

   - one active incident commander with watched decisions;
   - multi-party approvals before dangerous actions;
   - legal hold and evidence collection locks;
   - healthcare task handoffs with explicit owner leases;
   - emergency break-glass records with short TTL and audit export;
   - policy gates that combine human approval, passkey step-up, and delayed
     execution.

The common product principle is simple: Fiducia should coordinate who may act,
when they may act, what version they observed, and whether the result already
committed. Domain-specific systems still own domain truth.

### Hardening prerequisites for high-stakes tracks

The deeper the domain gets, the more Fiducia needs proof rather than slogans:

- persistent WAL, snapshots, compaction, restore drills, and monotonic
  fencing-token preservation;
- Jepsen-style tests for locks, KV, schedules, idempotency, and elections under
  partitions, crashes, restarts, and clock weirdness;
- model checking or state-machine property tests for union locks, queues,
  idempotency records, and schedule claims;
- explicit threat models: crash-fault tolerant Raft vs Byzantine/adversarial
  settings;
- audit log export and signed transparency checkpoints;
- per-domain examples that show how downstream systems reject stale fencing
  tokens;
- clear product language distinguishing coordination infrastructure from
  identity proofing, financial custody, or public election certification.

## Submodule Branch Tracking

Git supports `submodule.<name>.branch = .`, meaning "use the same branch name as
the current superproject branch" for submodule remote updates. See the
[Git submodule docs](https://git-scm.com/docs/git-submodule).

That could make feature-branch workflows cleaner than rewriting every
`.gitmodules` entry to `feature/foo`. For release pins, explicit `main`/`dev`
is still clearer.
