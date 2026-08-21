# Canonical private quote API staging

This branch stages the dedicated quote API as an internal Kubernetes service
behind the authenticated Canonical web tier. It does **not** create or authorize
`api.canonical.plus`, a public API origin, a Cloudflare route, DNS, R2, database,
or secret-store writes.

## Selected staging boundary

Current API main `f57528ccf2b077644917c1f770c97eca3027b8e7` authenticates every quote
REST and WebSocket request with both:

- `x-canonical-internal-token`, backed by `CANONICAL_INTERNAL_AUTH_TOKEN`;
- `x-canonical-subject`, projected by the trusted web tier only after it verifies
  the Canonical Shared Auth browser credential.

The API does not independently verify a Shared Auth bearer. It therefore runs as
one-replica `ClusterIP` service with NetworkPolicy ingress only from the
Canonical web pods. There is no API Ingress and no ingress-nginx or remote-gateway
allow rule.

DEN-2655 remains the product decision for a future public API: either retain
this BFF/private boundary or add independent origin-side bearer verification and
certify API, interfaces, clients, edge, CORS, revocation, and WebSockets together.
Never put the internal service credential in Cloudflare and never trust projected
identity headers as the sole origin authorization proof.

## Immutable release inputs

| Process | Source or release | Immutable image |
| --- | --- | --- |
| Quote API | `canonical-api-server.rs@f57528ccf2b077644917c1f770c97eca3027b8e7` | `ghcr.io/canonical-cloud/canonical-api-server.rs@sha256:f0706b58e791dee6cb6b7fcce8a109760ecfe1483ce8937ac6a4c76c9b29259b` |
| Web | monorepo release `d6226363aae7d4ebc7a5084b10509c3d189749b4` | `ghcr.io/canonical-cloud/canonical-web-server@sha256:0eac454163bc72bf12ba6659659d528e520d0e37eb473062add806b97c932b29` |
| Revoker | monorepo release `d6226363aae7d4ebc7a5084b10509c3d189749b4` | `ghcr.io/canonical-cloud/canonical-session-revoker@sha256:e32aba74453526a9a81a06d6e3e97f22e6adcc4e807d0284364b581ad0b8f39c` |

The web/revoker pair is the latest recorded attested release that introduced the
dedicated API boundary. Current web main is newer; a later promotion must replace
the pair atomically after the current monorepo composition publishes a new
attested release. Do not replace only one member of the pair.

## Secret boundary

The API ExternalSecret reads only these properties from
`dd/remote-dev/canonical-cloud-api`:

- `DATABASE_URL` — non-owner, non-superuser, non-`BYPASSRLS` runtime role;
- `GEMINI_API_KEY` — server-side provider credential;
- `CANONICAL_INTERNAL_AUTH_TOKEN` — at least 32 random bytes, shared only with
  the Canonical web runtime.

The web ExternalSecret maps that one token property to both
`CANONICAL_WEB_SERVICE_TOKEN` and `CANONICAL_INTERNAL_AUTH_TOKEN`. The first name
supports the attested d622 web image; the second is the current-source name. The
values must be identical during the compatibility window. No migration database
URL, Supabase service-role key, Shared Auth signing key, or introspection secret
is mounted into either runtime.

## Database gate

Before any sync:

1. provision separate migration and runtime PostgreSQL identities;
2. require the runtime role to be non-owner, non-superuser, and non-`BYPASSRLS`;
3. apply `canonical-api-server.rs/db/schema.sql` using the migration identity;
4. reconcile duplicate active contexts, then seed exactly one active context row
   for the intended Canonical owner and `quote-analysis` policy;
5. remove the migration identity from every long-lived workload;
6. certify forced RLS and cross-owner denial in the target database.

## Shared Auth gate

The web verifies the host-only `__Host-canonical-customer-auth` cookie against:

```text
http://dd-shared-auth-canonical-plus.shared-auth.svc.cluster.local:8120/auth/verify
```

Before sync, the current Canonical Shared Auth overlay must be deployed with an
independent issuer, audience, Supabase project, PostgreSQL identity/session
plane, signing and browser-sealing keys, Redis namespace, and cookie namespace.
The web NetworkPolicy allows only the pod carrying
`app.kubernetes.io/instance=canonical-plus` on port 8120.

## Model gate

The source default remains `gemini-3.6-pro`. Do not silently substitute another
model. Query the authenticated model inventory for the selected Canonical Google
project and region and require the exact identifier before enabling quote
analysis. Until then, health may be inspected but production quote analysis must
remain fail-closed.

## Replica gate

The API starts with one replica because status-event broadcast is process-local.
PostgreSQL and REST are authoritative, but two replicas can cause a WebSocket to
miss another pod's in-process event. Scale above one only after Redis/NATS/Postgres
notification fan-out or an equivalent cross-replica contract is implemented and
certified.

## Activation sequence

1. Require all workflow and repository checks on the exact head.
2. Prove the three immutable images and their attestations.
3. Provision and validate the exact External Secrets values without exposing
   them in GitHub or logs.
4. Apply and certify the database schema, roles, one active context, and RLS.
5. Deploy and certify the Canonical Shared Auth realm.
6. Render the overlay and inspect the complete diff against the intended cluster.
7. Keep `remote/argocd/apps/canonical-cloud.application.yaml` in manual mode.
8. Perform one explicit operator sync.
9. Test in-cluster web→API authentication, invalid-token rejection, subject
   isolation, quote submission, REST recovery, and WebSocket reconnect.
10. Test browser redirect, magic link/OTP, sealed return, CSRF, refresh, and
    revocation through `app.canonical.plus` only after Cloudflare and origin
    inventory is complete.

## Rollback

Record the currently deployed web/revoker digests and whether the API resources
exist before sync. Roll back by Git revert and an explicit Argo sync. Do not use
imperative image changes, delete an unknown Cloudflare/DNS resource, or roll back
a database migration without a separately reviewed data plan.
