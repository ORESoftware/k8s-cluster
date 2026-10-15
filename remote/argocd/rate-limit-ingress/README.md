# Rate-limit ingress overlays

These templates implement the load-balancer layer of the cross-organization
`ores-rate-limit` program. They are deliberately **not referenced by an Argo CD
ApplicationSet, root kustomization, or live cluster overlay**. Merging this
directory therefore changes no traffic. An application must explicitly select
one controller component and one reviewed policy ID before enforcement begins.

The application and authorization layers live in
`ORESoftware/ores-middleware` and `shared-auth`; distributed short-hot-set block
hints live in `ores-redis-lru-cache`; telemetry conventions live in
`ores-otel`. The ingress layer is an anonymous abuse/overload shield, not the
canonical account quota ledger.

## Controller choices

| Controller | Template | Scope | Intended use |
| --- | --- | --- | --- |
| ingress-nginx | `nginx/` | client IP, per controller replica | Existing ingress clusters; inexpensive short-window flood protection |
| Envoy Gateway | `envoy-gateway/` | route-wide, per Envoy instance | Gateway API clusters; overload protection with native HTTP 429 responses |
| HAProxy Ingress | `haproxy-ingress/` | client IP, peer-summed when peer tables are configured | Clusters that require cross-controller aggregation without querying Redis per request |

Exactly one ingress-controller template may target a route. Layering two local
controllers makes limits hard to reason about and is rejected by review.

## Why authenticated identity is absent here

No template keys on email, subject, organization, session, device, API key,
`Authorization`, cookies, or a caller-supplied identity header. Those values can
be spoofed before authentication and are too sensitive for ingress logs and
stick tables. The trusted origin derives a namespaced/versioned HMAC principal
through `ores-middleware` after `shared-auth` establishes canonical identity.

At this layer only the controller's verified client address, route, and method
may participate. Forwarded addresses are accepted only through the cluster's
explicit trusted-proxy chain. NAT-heavy routes should use conservative limits
and application-side authenticated quotas to avoid penalizing unrelated users.

## Activation procedure

1. Start with Cloudflare/edge and application decisions in audit/shadow mode.
2. Choose one directory below and copy it into the adopting application's own
   environment overlay. Do not add this catalog directly to a global
   ApplicationSet.
3. Replace every `replace-me-before-enabling` target and use the stable policy
   label `rate-limit.ores.io/profile: public-anonymous-ingress` on the intended
   Ingress or HTTPRoute only.
4. Recalculate local limits for the maximum replica count. NGINX and Envoy local
   budgets are per proxy instance; a nominal 20 requests/second across three
   replicas can admit approximately 60 requests/second.
5. For HAProxy peer-summed mode, verify controller peers are healthy before
   activation. Otherwise treat the limit as per replica.
6. Run the Rust validator and the adopting repository's rendered-manifest tests.
7. Observe `ores-otel` aggregate decisions and saturation for at least one
   release before tightening values.
8. Keep strict multi-node account, billing, login, OTP, recovery, and long-window
   policies in the distributed/application or authorization layer.

## Template policy

The catalog profile is intentionally modest and illustrative:

- policy ID: `public-anonymous-ingress`;
- 20 requests/second;
- burst multiplier 5 where supported;
- 10 concurrent connections where supported;
- no whitelist;
- activation label: `disabled-template`;
- no Redis address, credential, secret, or raw identity value.

The activation label documents lifecycle only; it is not a controller feature.
Safety comes from this catalog not being included in live GitOps and from the
Envoy target name being deliberately nonexistent. Inclusion is an explicit
security change requiring a normal pull request.

## Controller semantics

### ingress-nginx

The native `limit-rps`, `limit-connections`, and burst annotations use shared
memory inside one controller pod. They avoid a Redis round trip but do not form a
global quota across replicas. The controller's configured rejection status may
differ from the application contract; do not interpret an ingress rejection as
an account-level quota decision.

### Envoy Gateway

`BackendTrafficPolicy` local rate limiting is route-local and instance-local.
The template deliberately leaves `clientSelectors` unset, producing a route-wide
overload budget rather than trusting a user header. Distinct user/IP buckets
require the reviewed global rate-limit service or the application middleware.

### HAProxy Ingress

The native annotations limit per client IP. HAProxy peer stick tables can sum
rates across controller replicas when peer synchronization is configured and
healthy. The validator models both per-replica and peer-summed states so an
operator cannot describe peer-summed behavior without explicitly acknowledging
the peer dependency.

## Validation and invariants

`tools/rate-limit-ingress-validator` uses closed Rust enums and exhaustive
matches for controller, scope, and activation state. CI checks all three
manifests for:

- disabled-template lifecycle marker;
- stable policy ID;
- only the expected controller-specific primitives;
- no secret-bearing Kubernetes resource;
- no Redis endpoint;
- no email, auth header, cookie, API-key, subject, session, organization, or
  device selector;
- valid controller/scope combinations;
- an explicit non-existent Envoy target until adoption.

The bounded state test enumerates every controller × scope × activation
combination. Unsupported combinations remain errors rather than silently
changing semantics.
