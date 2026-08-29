# Bounded tRPC and GraphQL entrypoints for the Connecting Rooms NestJS service

Status: proposed; target-repository implementation is blocked on repository access

Decision date: 2026-08-23

Tracking: [ORESoftware/k8s-cluster#1402](https://github.com/ORESoftware/k8s-cluster/issues/1402), [DEN-3852](https://linear.app/denman/issue/DEN-3852/connecting-rooms-add-single-path-trpc-and-graphql-query-param-routes)

Target repository: `connecting-rooms/connecting-rooms`

## Triage and evidence gate

DEN-3852 identifies `connecting-rooms/connecting-rooms` as the intended NestJS
service and asks for fixed `/trpc` and `/graphql` GET compatibility surfaces. As of
2026-08-23, the GitHub App available to this workstream cannot resolve that
repository. The repository returns `404` through the installed integration and no
connected mirror is available.

Consequently, the exact recent commit, current NestJS and transport versions,
router/schema structure, existing auth guards, and test commands are **not yet
verified**. An attachment on DEN-3852 may be used as non-authoritative design input,
but it must not be applied or represented as current source before it is reconciled
against a readable repository checkout.

Before an implementation PR is opened, its author must record all of the following
on #1402 and DEN-3852:

1. the exact repository URL and default branch;
2. the full commit SHA of the recent NestJS change named by the request;
3. the inspected NestJS, tRPC, GraphQL adapter, and HTTP platform versions;
4. the existing request-context, authentication, tenant, validation, error, rate,
   timeout, docs, and telemetry boundaries that the adapters will reuse; and
5. the repository-native lint, unit, integration, and end-to-end commands.

Missing evidence fails closed. It is not permission to guess paths, package
versions, or security behavior.

## Context

The requested shape places a procedure or GraphQL document in query parameters on
one fixed path. That is convenient for callers, but a general query tunnel can make
procedure discovery, authorization, caching, rate limits, observability, and cost
controls materially weaker than ordinary registered routes.

This decision permits compatibility entrypoints only when they dispatch to a closed
registry and reuse the application's existing domain boundary. The adapters are not
a second application API and may not reach repositories, databases, provider clients,
or tenant state directly.

## Decision

The Connecting Rooms service may expose two bounded compatibility routes:

- `GET /trpc?path=<registered-query>&input=<url-encoded-json>`; and
- `GET /graphql?query=<allowlisted-document>&variables=<url-encoded-json>&operationName=<name>`.

Both are query-only compatibility surfaces. The canonical tRPC adapter remains the
preferred interface for native tRPC clients, and persisted GraphQL operations are the
preferred production interface. A deployment may omit either compatibility route
when no reviewed caller needs it.

### Transport comparison

| Shape | Benefit | Principal risk | Decision |
| --- | --- | --- | --- |
| Fixed `/trpc` with `path` and `input` query parameters | Matches the requested single-path caller contract | A dynamic path resolver can expose object properties or procedures that were never intended for GET | Allowed only behind a static query-procedure registry, strict decoding, and the controls in this document |
| Canonical tRPC `/trpc/<procedure>` with encoded `input` | Uses the supported tRPC request shape, normal envelopes, batching policy, context, and error hooks | Still needs per-procedure authorization, input limits, timeout, and rate controls | Preferred for native tRPC clients; do not replace it with an unbounded compatibility shim |
| `/graphql` with arbitrary `query` text | Standard GraphQL-over-HTTP client shape and strong development ergonomics | Public callers can select novel, expensive, deeply nested, or weakly observed operations | Development-only; disabled in production by default |
| `/graphql` with persisted or allowlisted operations | Stable operation identity supports authorization, cost, caching, and telemetry | An automatic persisted-query cache can become an allow-all learning cache if registration is unrestricted | Required in production; deploy an explicit reviewed safelist rather than treating APQ alone as authorization |

## One application boundary

The Nest request context is created once per HTTP request and passed into the selected
transport adapter. It contains only the application's established, request-scoped
identity, tenant, authorization service, request ID, abort signal, and domain-service
facades. It must not contain a privileged global database handle merely because the
new adapter needs context.

Every registered tRPC procedure and GraphQL resolver must call the same application
service method used by existing HTTP behavior. That method owns:

- authentication and credential/session assurance;
- tenant selection and tenant-membership checks;
- product authorization for the requested resource and action;
- schema validation, normalization, and domain invariants;
- transaction and side-effect ownership;
- stable domain-error classification; and
- audit events where the existing application requires them.

The transport layer owns decoding, transport-specific validation, response envelopes,
and mapping the stable domain error to the existing public error policy. A resolver or
procedure may not weaken an authorization failure, accept a tenant from caller input
without comparing it to authenticated context, or return raw internal exceptions.

## tRPC compatibility contract

The fixed `/trpc` route has these invariants:

1. `path` is required, decoded exactly once, 1–128 ASCII characters, and matches
   `^[A-Za-z0-9_.-]+$`.
2. Dispatch is an own-property lookup in an immutable, explicitly exported registry
   of query procedures. It never walks an object graph, evaluates text, imports a
   module dynamically, or derives a method name from unchecked input.
3. Mutations and subscriptions are absent from this registry. A request for either is
   rejected with `405 Method Not Allowed` and an `Allow` header naming the supported
   non-GET method when such a route exists.
4. `input` is optional only for procedures whose typed input permits absence. When
   present, it is decoded once as UTF-8 JSON, is limited to 8 KiB after percent
   decoding, and must pass the procedure's existing runtime schema.
5. Malformed percent encoding, malformed JSON, duplicate singleton parameters,
   unknown procedures, and invalid input produce deterministic client errors. They do
   not reach the domain service and never echo raw input.
6. Successful and error responses use the installed tRPC version's normal envelope
   and content type. The compatibility adapter does not invent a second result shape.
7. Batching is disabled unless a later review gives it an aggregate procedure count,
   aggregate input size, complexity budget, and atomicity/error contract.

The compatibility route must be implemented as a small adapter around the installed
tRPC router/context facilities after their versions are verified. It is not a fork of
tRPC request execution.

## GraphQL compatibility contract

The fixed `/graphql` route follows the GraphQL-over-HTTP GET parameter names:
`query`, optional `variables`, and optional `operationName`. Its invariants are:

1. GET executes queries only. A parsed mutation or subscription is rejected with
   `405 Method Not Allowed` and an `Allow` header naming the service's supported
   non-GET method. Operation type is checked after parse and before execution.
2. `variables`, when present, is a JSON object no larger than 8 KiB after percent
   decoding. Malformed JSON, a non-object value, duplicate singleton parameters, or
   invalid variable coercion produces a deterministic client error before resolver
   execution.
3. `operationName` is at most 128 characters and is required when the document
   contains multiple named operations.
4. In production, an operation must match a reviewed registry entry containing a
   stable operation ID, operation name, normalized document, SHA-256 document digest,
   operation type, authorization policy, and maximum cost. Unknown IDs, names, or
   digests fail closed.
5. If production accepts the `query` parameter for compatibility, the normalized
   document digest must already exist in that registry. Receiving text does not
   register it. Prefer a stable persisted-operation ID for callers that support one.
6. Automatic persisted queries may optimize transfer, but an APQ cache that learns
   arbitrary documents is not the production safelist and grants no authorization.
7. Introspection is disabled in production by default. A separately authenticated
   operator policy may enable it for an explicit environment and expiry; obscurity is
   not relied upon as an authorization control.
8. The existing Nest GraphQL schema, resolvers, guards, interceptors, request context,
   and error formatter remain authoritative. The compatibility adapter does not build
   a parallel schema or invoke resolvers outside normal execution.

Development may accept arbitrary GraphQL query text only under a non-production,
explicitly configured mode with the same authentication, tenant, depth, complexity,
payload, timeout, and response limits. The production default is fail-closed even if
the environment variable controlling the mode is missing or malformed.

## Required production limits

The implementation may set stricter product-specific values after measurement. It may
not silently raise these initial ceilings; a higher ceiling requires review and tests.

| Control | Initial ceiling | Enforcement point |
| --- | --- | --- |
| Entire request target | 16 KiB | HTTP adapter, before query parsing |
| Decoded tRPC `input` | 8 KiB | Compatibility decoder, before JSON parse |
| Decoded GraphQL `variables` | 8 KiB | Compatibility decoder, before JSON parse |
| tRPC path / GraphQL operation name | 128 characters | Parameter validator |
| GraphQL document depth | 8 | Validation rule, before execution |
| GraphQL complexity | 100 weighted units | Validation rule with field/list costs, before execution |
| GraphQL aliases | 20 | Validation rule, before execution |
| GraphQL root fields | 10 | Validation rule, before execution |
| Caller-controlled page size | 100 records | Typed input/schema and domain service |
| Request execution | 5 seconds | Shared deadline and cancellation signal |
| Serialized response | 1 MiB | Bounded response writer |
| Per identity + tenant + operation | 60 requests/minute, burst 10 | Existing distributed rate limiter before execution |

Anonymous access, if the existing product deliberately supports it, uses a separately
reviewed network/device bucket and a lower ceiling; it does not collapse all anonymous
traffic into a bypass. Rate-limit keys are derived from trusted authenticated context,
not caller-supplied tenant or operation labels.

Timeout must propagate cancellation through resolvers/procedures and owned downstream
calls. A timeout response does not imply that an uncancelled mutation may continue; GET
surfaces are read-only regardless. The application must continue to enforce database,
provider, pagination, and fan-out bounds below the transport layer.

The HTTP stack must retain its established CSRF, origin, cookie, and same-site policy.
GET is never used for a state-changing operation, including hidden cache fill, login,
subscription creation, or provider mutation. Cache headers must reflect authenticated
and tenant-specific data; shared caching is disabled unless the response has an
explicit safe cache design.

## Errors and status behavior

Transport errors are deterministic and payload-free in logs:

- malformed percent encoding or JSON: `400`;
- syntactically invalid GraphQL document: `400`;
- valid GraphQL syntax that fails validation/coercion: the installed server's reviewed
  GraphQL-over-HTTP-compatible client-error response;
- mutation/subscription attempted over GET: `405` with `Allow`;
- missing authentication: the service's existing unauthenticated mapping;
- authenticated but unauthorized or cross-tenant request: the existing fail-closed
  mapping without resource-existence disclosure;
- unknown or non-allowlisted operation: stable client error without enumerating the
  registry; and
- timeout/rate/payload failure: the existing bounded public error shape and applicable
  retry metadata.

Raw exceptions, stack traces, query text, input, variables, tokens, cookies, and tenant
or user data never enter a public error.

## Documentation and telemetry

The compatibility HTTP route declarations, validation schemas, runtime registration,
and OpenAPI 3.1 output must share one typed source of truth. The service documents the
two GET surfaces, parameter encodings, limits, authentication, error envelopes, and
GET query-only behavior at its existing API documentation paths. GraphQL SDL and the
persisted-operation registry remain the GraphQL operation sources of truth; OpenAPI
documents the HTTP compatibility envelope rather than pretending to describe every
GraphQL field.

CI regenerates and checks route/OpenAPI artifacts. Any generated SDK change is reviewed
in the same PR, with the source commit and contract digest recorded according to the
portfolio API contract.

Instrumentation occurs explicitly at the owned request/transport boundary. Allowed
low-cardinality dimensions are transport, stable registered procedure or operation ID,
allowlist digest/version, outcome class, status class, and bounded latency/complexity
buckets. Request and trace IDs remain fields rather than metric or Loki labels.

Telemetry must not contain raw procedure input, GraphQL text, variables, response
payloads, authorization headers, cookies, credentials, user/tenant/resource IDs, or
raw exception messages. Unknown operations use one bounded label such as `unknown`;
never use attacker-supplied names as metric labels.

## Verification matrix

The repository-specific implementation PR must add automated tests for every row:

| Area | Required evidence |
| --- | --- |
| Triage | Exact repository, recent commit SHA, dependency versions, existing request-context path, and commands are recorded |
| tRPC success | Registered query receives the real Nest request context and normal tRPC envelope |
| tRPC registry | Unknown path, inherited-property name, mutation, subscription, dynamic traversal, malformed encoding/JSON, duplicate parameter, oversize input, and schema-invalid input all fail before domain execution |
| GraphQL success | One allowlisted query reaches the existing schema/resolver/guard/context and returns the installed server's normal response |
| GraphQL safelist | Unknown ID/hash/name, text-registration attempt, document/hash mismatch, anonymous introspection, and arbitrary production text fail closed |
| GraphQL method | Query works over GET; mutation and subscription over GET return `405` plus `Allow` and execute no resolver |
| GraphQL parsing | Malformed query, malformed/non-object variables, duplicate parameters, absent/unknown `operationName`, and invalid coercion are deterministic |
| Cost controls | Depth 8 passes and 9 fails; complexity 100 passes and 101 fails; alias 20 passes and 21 fails; root-field 10 passes and 11 fails; page-size 100 passes and 101 fails |
| Shared policy | Existing unauthenticated, unauthorized, cross-tenant, validation, not-found, conflict, and internal-error cases match the established service behavior on both transports |
| Bounds | Request-target, decoded-payload, rate/burst, five-second timeout/cancellation, downstream fan-out, and one-MiB response limits are exercised at the boundary and one unit beyond it |
| CSRF/cache | Credentialed GET retains the existing origin/same-site protection, no GET operation changes state, and tenant responses cannot enter an unsafe shared cache |
| Docs | Runtime routes equal the generated OpenAPI method/path set; docs describe encodings/limits; GraphQL schema and operation-registry generation are deterministic |
| Telemetry | Known operation emits bounded identifiers; unknown/adversarial values do not become labels; logs contain none of the submitted input/query/variables/token fixtures |
| Regression | Existing REST/Nest, resolver, auth, tenant, OpenAPI, and telemetry suites remain green |

Tests use in-process Nest HTTP requests with the real middleware/guard/interceptor
stack, not direct calls that manufacture a trusted context. Spies at the application
service boundary prove that rejected inputs execute no domain operation.

## Rollout and rollback

1. Restore least-privilege GitHub App access to the exact target repository with
   contents and pull-request read/write plus Actions visibility. Do not use a pasted
   personal access token.
2. Complete the triage evidence gate and reconcile any DEN-3852 attachment against
   the exact target commit.
3. Land shared application-boundary tests before registering either new route.
4. Deploy to a non-production environment with both compatibility routes disabled;
   enable one route at a time for named test callers.
5. Exercise the verification matrix, inspect redacted telemetry and rate/timeout
   behavior, and review the production operation registry.
6. Enable a small production caller cohort, then expand only after error, latency,
   denial, and cost budgets remain within product thresholds.

Each route has an independent fail-closed feature flag. Disabling a flag unregisters
the route or returns a stable unavailable response before decoding caller input. The
canonical tRPC route, existing REST routes, and supported non-GET GraphQL path do not
depend on either compatibility flag. Rollback disables the affected flag and restores
the prior immutable application artifact; it does not broaden the GraphQL safelist or
temporarily turn off auth, tenant, cost, or CSRF controls.

This ADR may move from proposed to accepted only after the repository owner and a
security reviewer approve the recorded implementation evidence. #1402 remains open
until the repository-specific PR exists and every acceptance criterion has linked
test evidence.

## Primary references

Retrieved 2026-08-23:

- [tRPC Express adapter](https://trpc.io/docs/server/adapters/express)
- [tRPC server context](https://trpc.io/docs/server/context)
- [NestJS GraphQL complexity controls](https://docs.nestjs.com/graphql/complexity)
- [NestJS GraphQL guards and request context](https://docs.nestjs.com/graphql/other-features)
- [NestJS GraphQL quick start](https://docs.nestjs.com/graphql/quick-start)
- [GraphQL over HTTP working draft](https://graphql.github.io/graphql-over-http/draft/)
- [Apollo Server automatic persisted queries](https://www.apollographql.com/docs/apollo-server/performance/apq)
