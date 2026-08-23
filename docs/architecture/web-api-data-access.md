# ADR: web-to-API data-access paths

- **Status:** accepted portfolio default
- **Decision date:** 2026-08-23
- **Tracking:** [ORESoftware/k8s-cluster#1399](https://github.com/ORESoftware/k8s-cluster/issues/1399), [DEN-3960](https://linear.app/denman/issue/DEN-3960/document-4-web-server-to-api-server-data-access-patterns-across-10)
- **Related caching work:** [ORESoftware/k8s-libs-and-shared-defs#23](https://github.com/ORESoftware/k8s-libs-and-shared-defs/issues/23)

## Context

The portfolio has traditional browser-facing web servers, API servers, and a few combined
services. A web tier can reach product data through several valid paths, but an implicit choice
creates duplicate authorization, accidental write authority, unbounded retry, and unclear failure
ownership. This ADR requires an explicit path for every web-to-domain operation.

The API tier owns product writes and business invariants by default. A web process may own narrowly
scoped browser-tier state such as encrypted sessions, but that exception does not make it a product
data writer. Declarative schemas and migrations remain in the product's interface or shared
`*-lib-core` contract; route handlers and runtime ORM models are consumers, never competing schema
authorities.

This decision applies to traditional web/API pairs. Fiducia brain, node, routing, and other
coordination-plane services are outside this analysis.

## Decision

Every operation selects exactly one primary path. A service can use several paths, but each route,
query, command, or subscription must have one named owner and one documented exception record.

### P1 — direct read-only database query

Use P1 only for measured read paths whose tenant and authorization policy can be enforced at the
database boundary without duplicating a business invariant.

- The web runtime receives a distinct login with `CONNECT`, schema `USAGE`, and an explicit
  `SELECT` allowlist only. It has no DML, DDL, sequence, function-execution, ownership,
  `CREATEROLE`, or `BYPASSRLS` capability.
- Production startup verifies the exact expected login, `default_transaction_read_only=on`, and the
  required RLS or equivalent tenant policy before accepting traffic. A generic "not superuser"
  check is insufficient.
- Tenant, subject, and policy context come from verified authentication, never request fields. The
  transaction sets that context locally before any query. Where a database has no RLS, the shared
  named-query layer must make the verified tenant/owner predicate structurally mandatory.
- Handlers receive only named, policy-aware query functions from the shared read-only ORM boundary;
  they do not receive a raw connection, entity manager, SQL string, or write-capable type.
- Pool checkout, statement time, result rows, and response bytes are bounded. Only transient reads
  may be retried, within the request deadline and with jitter.
- P1 provides database-level consistency and the lowest service-hop latency, but it couples the web
  release to schema compatibility and expands database connection pressure. It fails closed when
  identity, policy context, privilege verification, or schema compatibility is unavailable.

P1 must not perform product writes. A web-owned session store is a separately named authority with
its own credential, schema/grant boundary, and migration path; it is not a loophole for domain DML.

### P2 — stateless HTTP request to the API cluster

P2 is the default for commands, product writes, reads that require business-policy evaluation, and
ordinary request/response queries where the extra hop is acceptable.

- The web tier calls a typed, versioned API/SDK over authenticated TLS. Browser credentials are not
  blindly forwarded; the API validates an audience-bound user or delegated service token and
  enforces product authorization itself.
- Every call has connect, first-byte, whole-request, and response-size bounds. Trace context and a
  non-personal correlation ID propagate across the hop.
- Safe reads may retry transient connection failures within the original deadline. Writes retry only
  with a stable operation/idempotency key, and the API durably returns the original result for a
  replay. Neither side retries validation, authorization, or deterministic conflict responses.
- Client pools cap open and idle connections. API admission control, per-tenant quotas, and explicit
  `429`/`503` plus bounded `Retry-After` provide backpressure.
- P2 keeps authorization, transaction boundaries, and business invariants in one tier and isolates
  the database from the public web process. Its cost is one extra network hop and dependence on API
  availability. The web tier returns a bounded error or deliberately stale safe view; it never falls
  back to a more privileged database path.

HTTP keep-alive is still P2. Connection reuse alone does not create a stateful application protocol.

### P3 — bounded stateful connection to the API cluster

Use P3 only for sustained subscriptions, bidirectional coordination, or high-rate ordered streams
where repeated P2 calls materially harm latency or lifecycle correctness. WebSocket, HTTP/2 streams,
and a framed TCP protocol are examples; an unframed ad-hoc socket is not.

- Authentication and authorization occur at handshake and are revalidated on policy/session expiry.
  The protocol is versioned and has bounded frame sizes, queues, in-flight operations, and per-tenant
  connection counts.
- Each command has a stable operation ID and deadline. Sequence or cursor values make reconnect and
  replay explicit. Acknowledgements mean a documented durability level, not merely socket receipt.
- Heartbeat, idle timeout, maximum connection lifetime, reconnect jitter, and circuit breaking are
  mandatory. Deployments stop admission, signal drain, finish or reject bounded in-flight work, and
  publish a last resumable cursor before termination.
- Producers cannot block unboundedly behind a slow consumer. The contract chooses coalescing,
  dropping with a resync marker, or closing with an explicit overload reason.
- Handshake and connection spans link bounded per-operation spans. Metrics use route/protocol/outcome
  labels, never tenant, subject, connection, or payload identifiers.
- P3 amortizes handshakes and supports ordered low-latency delivery, but adds connection affinity,
  rolling-deploy drain, replay, and partial-partition complexity. If resumability cannot be proven,
  prefer P2 or P4.

### P4 — NATS/message-queue command with asynchronous result

Use P4 for durable work whose result need not complete inside the initiating HTTP request: imports,
fan-out, media processing, provider reconciliation, alert delivery, and other load-leveling jobs.

- Publish a versioned envelope containing an opaque event/operation ID, type, schema version,
  deadline, correlation and causation IDs, `traceparent`, and the minimum authorized payload. Do not
  put credentials or unnecessary personal data in subjects, headers, or telemetry.
- A database mutation and outbox insertion commit in one transaction. A consumer claims the
  operation ID in the same transaction as its side effect, acknowledges only after the declared
  durability point, and returns the stored result for duplicate delivery.
- Publish and request/reply waits are bounded. The message has an expiry/deadline; consumers do not
  begin stale work. Delivery attempts, exponential backoff, maximum acknowledgement pending,
  per-consumer concurrency, and a dead-letter/advisory path are explicit.
- Results use a durable status resource or a scoped reply subject with correlation and expiry. A lost
  transient reply must not cause the command to run twice; callers can query the stable operation ID.
- Consumers drain subscriptions and bounded in-flight work on shutdown. Queue depth, oldest-message
  age, redelivery, dead-letter, duration, and outcome are observable with fixed-cardinality labels.
- P4 decouples availability and absorbs bursts, but is eventually consistent and introduces
  duplicate delivery, ordering, poison-message, and operator-recovery concerns. It is not a hidden
  synchronous RPC path.

## Selection matrix

| Question | P1 direct read | P2 stateless HTTP | P3 stateful API connection | P4 asynchronous queue |
| --- | --- | --- | --- | --- |
| Product writes | Forbidden | Default | Allowed only as idempotent framed commands | Preferred for durable asynchronous commands |
| Read consistency | Strong or explicitly bounded database view | API-defined | Cursor/sequence-defined | Eventual; query durable result separately |
| Typical latency | Lowest hop count | One API hop | Lowest sustained-stream overhead | Admission fast; completion asynchronous |
| Authorization | DB role plus RLS/mandatory named predicate | API policy authority | API policy at handshake and operation | API/consumer policy plus subject ACL |
| Retry identity | Read request | HTTP idempotency key for writes | Operation ID plus sequence/cursor | Durable event/operation ID plus inbox |
| Backpressure | Pool, statement, and row limits | Admission, connection, and request limits | Bounded frames, queues, and connections | Stream/consumer limits and max delivery |
| Primary failure risk | Schema/role coupling and connection exhaustion | API unavailability and retry storms | Partition, affinity, replay, and drain errors | Lag, duplicates, poison messages, lost transient replies |
| Choose when | A narrow read is measurably worth the coupling | Ordinary queries and all synchronous commands | A real sustained session justifies lifecycle cost | Completion can be asynchronous and durable |

If two choices appear equally suitable, choose P2. Move to P1 only with read-only privilege and
authorization evidence, to P3 only with lifecycle/replay evidence, and to P4 only with durable
idempotency and operator-recovery evidence.

## Cross-cutting contracts

### Schema and migration ownership

Each organization names one declarative schema authority—normally its `*-interfaces` or
`*-lib-core` repository, or the appropriate slice of
`ORESoftware/k8s-libs-and-shared-defs`. DPM or the product's reviewed declarative migration tool
produces ordered SQL for a human-reviewed, discrete migration job. API and web runtime replicas do
not synthesize DDL from ORM types and do not run shared migrations at startup. The API-owned release
sequence applies compatible migrations before code that depends on them and retains an explicit
rollback/forward-fix plan.

### Timeouts, idempotency, and drain

One end-to-end deadline is established at ingress and propagated or shortened downstream. Local
timeouts do not extend it. Every product write has a stable operation ID whose authorization scope
and normalized request digest are checked on replay. Reusing an ID with a different principal,
tenant, operation, or body is a conflict. All client pools, servers, streams, and consumers stop new
admission and drain a documented bounded amount of in-flight work during deployment.

### Observability and privacy

W3C trace context crosses every selected path. Spans record path (`p1`–`p4`), stable operation name,
outcome, and bounded timing; metrics record fixed-cardinality service/route/path/outcome labels.
Logs may contain an opaque correlation or operation ID but not tokens, cookies, database URLs,
raw payloads, tenant/user identifiers, or message contents. Health checks remain dependency-free;
readiness checks the dependencies required by the paths enabled for that replica.

### Caching

Caching does not change path ownership or authorization. Cache keys include tenant, subject or
authorization scope, policy/version, representation, and schema dimensions as required by the
shared caching plan. Notifications are wake-up hints, not the only invalidation authority. A cache
failure cannot fall back from P2/P3/P4 to an undeclared P1 credential.

## Organization adoption records

The following source-of-truth repositories must carry an org-local adoption document and a comment
at the actual transport/database boundary. The local record names the current state, selected
default, exceptions, schema/migration owner, and path-specific controls. Links target `main` so they
become durable after each review is merged; implementation PRs are tracked on issue #1399.

| Organization | Adoption repository | Selected default and principal exceptions |
| --- | --- | --- |
| `sonus-auris` | [`sonus-auris-web-server.rs`](https://github.com/sonus-auris/sonus-auris-web-server.rs/blob/main/docs/web-api-data-access.md) | P2 for domain reads/writes; separately scoped web-session store; future P1 only through named read-only ORM queries |
| `zed-pkg` | [`zed-web-server.rs`](https://github.com/zed-pkg/zed-web-server.rs/blob/main/docs/web-api-data-access.md) | P1 for registry views; P2 for publish/admin writes; P3/P4 only for explicit streaming/worker contracts |
| `quaestor-ledger` | [`quaestor-web-server.rs`](https://github.com/quaestor-ledger/quaestor-web-server.rs/blob/main/docs/web-api-data-access.md) | P1 for tenant-scoped ledger views and web-owned sessions; P2 for API-owned commands; P3 wakeups never replace durable pull |
| `daedalus-fab` | [`daedalus-web-server.rs`](https://github.com/daedalus-fab/daedalus-web-server.rs/blob/main/docs/web-api-data-access.md) | P1 for verified-operator read views; P2 for fabrication mutations; P3 run updates and P4 jobs only with explicit contracts |
| `fiducia-cloud` | [`fiducia-admin.rs`](https://github.com/fiducia-cloud/fiducia-admin.rs/blob/main/docs/web-api-data-access.md) | P2 for admin calls to auth/customer/brain APIs; no direct Raft/node database access; P3/P4 only for bounded operations protocols |
| `canonical-cloud` | [`canonical-web-server.rs`](https://github.com/canonical-cloud/canonical-web-server.rs/blob/main/docs/web-api-data-access.md) | P1 for named tenant reads and web sessions during API split; P2 for API-owned commands; P3 only as resumable wakeups |
| `cliptown` | [`cliptown-rust-backend.rs`](https://github.com/cliptown/cliptown-rust-backend.rs/blob/main/docs/web-api-data-access.md) | P2 is the public client/web boundary; the combined API service owns authorized transactions; async work graduates to P4 |
| `file-tunnel` | [`ftnl-web-server.rs`](https://github.com/file-tunnel/ftnl-web-server.rs/blob/main/docs/web-api-data-access.md) | P2 from the static portal to the control API; P3 for bounded transfer progress/data channels; the portal has no DB credential |
| `embedded-alerts` | [`eal-mash-web`](https://github.com/embedded-alerts/eal-mash-web/blob/main/docs/web-api-data-access.md) | P1 read-only alert views; P2 for rule/acknowledgement writes; P3 live wakeups and P4 delivery jobs |
| `evento-globolo` | [`evgl-mash-web`](https://github.com/evento-globolo/evgl-mash-web/blob/main/docs/web-api-data-access.md) | P1 read-only event views; P2 for event/ticket commands; P3 live wakeups and P4 import/cross-post jobs |

## Review checklist

An org-local adoption is complete only when reviewers can answer yes to all applicable items:

1. Every operation names P1, P2, P3, or P4, and fallback cannot silently change paths.
2. P1 evidence proves the exact read-only role and tenant/authorization enforcement.
3. P2/P3/P4 writes have stable idempotency, bounded timeouts, trace propagation, and replay rules.
4. P3 documents admission, heartbeat, reconnect, cursor/resume, backpressure, and deployment drain.
5. P4 documents outbox/inbox atomicity, delivery limits, expiry, dead-letter recovery, and durable result lookup.
6. The declarative schema owner, migration tool, privileged migration job, and runtime prohibition are named.
7. Logs, metrics, traces, subjects, and errors exclude credentials, raw payloads, and high-cardinality identities.
8. Tests exercise duplicate delivery/retry, timeout, policy denial, overload, drain, and dependency failure for enabled paths.

## Consequences

The portfolio retains a narrow, defensible direct-read option without normalizing browser-tier write
authority. Most synchronous behavior converges on typed HTTP; stateful connections and queues are
available when their lifecycle semantics are justified. The cost is explicit per-operation design,
separate credentials, more failure tests, and a coordinated rollout across repositories. That cost
is intentional: transport choice is part of the security and consistency contract, not an incidental
client-library detail.
