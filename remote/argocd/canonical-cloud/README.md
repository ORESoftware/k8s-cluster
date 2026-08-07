# canonical.plus application and API GitOps

This overlay is the dedicated deployment boundary for the canonical.plus Rust
application, API, and session-revocation worker. It is deliberately separate
from `dd-next-runtime`: the legacy `dd-canonical-cloud` workload remains
untouched until the prebuilt-image path has been activated and verified.

`canonical-cloud/canonical-monorepo` remains the only deployable source for
the web and revoker images. The dedicated quote API is released from
`canonical-cloud/canonical-api-server.rs` because that repository owns its
runtime, container contract, and immutable digest artifact. This overlay
consumes only reviewed immutable digests and never builds source in-cluster. The existing
`remote/deployments/canonical-cloud` secondary submodule is legacy operational
context and is not an input to this overlay.

Argo CD is the only runtime writer. GitHub Actions builds and attests the web
and revoker images, but it never deploys the umbrella `canonical.cloud` stack,
receives no kubeconfig, and never applies Kubernetes resources. A reviewed Git commit promotes exact registry digests here; Argo CD
then reconciles that commit from `k8s-cluster@dev`.

## Process and credential boundaries

- `canonical-cloud-web` runs `canonical-web-server serve`, exposes only port
  8081 through a ClusterIP Service, and is reachable only from
  `dd-remote-gateway` in the `default` namespace (plus the observability
  namespace). Its HTTP connection supports HTMX, REST, and WebSocket upgrades.
- `canonical-cloud-api` runs `canonical-api-server.rs` on 8080. Direct clients
  authenticate with a Canonical Supabase bearer token; the web process uses a
  separate random service credential plus its already verified shared-auth user
  id. The API persists owner-scoped quote records, combines the reviewed
  Markdown playbook with `canonical_context`, calls Gemini, and exposes
  owner-scoped REST and WebSocket updates.
`canonical-cloud/canonical-monorepo` is the only deployable source of truth. Its
exact tested commit produces all three release images; this overlay consumes
only immutable digests and never deploys the umbrella `canonical.cloud`
repository or an individual application repository. The existing
`remote/deployments/canonical-cloud` secondary submodule is legacy operational
context and is not an input to this overlay.

`canonical-cloud/canonical-monorepo` is the only deployable source of truth. Its
exact tested commit produces all three release images; this overlay consumes
only immutable digests and never deploys the umbrella `canonical.cloud`
repository or an individual application repository. The existing
`remote/deployments/canonical-cloud` secondary submodule is legacy operational
context and is not an input to this overlay.

Argo CD is the only runtime writer. GitHub Actions builds and attests the web,
API, and revoker images, but it receives no kubeconfig and never applies
Kubernetes resources. A reviewed Git commit promotes exact registry digests
here; Argo CD then reconciles that commit from `k8s-cluster@dev`.

## Host and process boundaries

- `app.canonical.plus` serves the Maud/HTMX browser application from
  `canonical-cloud-web`. Its `/api/v1/quotes` prefix, including
  `/api/v1/quotes/ws`, is routed to `canonical-api-server` so browser REST
  recovery and WebSocket status use the same API implementation as SDKs.
- `api.canonical.plus` routes to `canonical-api-server` for Shared Auth bearer
  REST and WebSocket clients. The API process independently reverifies the raw
  bearer against the Canonical Shared Auth realm and never treats
  Cloudflare-injected identity headers as proof.
- `app.canonical.plus/shared-auth/*` is owned by the separately deployed
  Canonical Shared Auth overlay in `shared-auth/shared-auth-server.rs`. Its
  NGINX route strips only the `/shared-auth` prefix before forwarding to the
  realm service.
- `canonical-cloud-revoker` runs `canonical-session-revoker run`. It declares no
  port, Service, Ingress, or accepted NetworkPolicy ingress.

The public Ingress terminates TLS for `app.canonical.plus` and
`api.canonical.plus`, preserves WebSocket `Upgrade` and `Connection` behavior,
and disables proxy buffering for status streams. Cloudflare may proxy these
hosts and attach the auth-edge Worker, but origin-side authentication remains
mandatory.

## Credential boundaries

`canonical-cloud-web-runtime`, `canonical-cloud-api-runtime`, and
`canonical-cloud-revoker-runtime` are different Kubernetes Secrets backed by
different AWS Secrets Manager objects. `canonical-cloud-ghcr-pull` is a fourth,
registry-only Secret. Never combine these objects.

The web AWS object `dd/remote-dev/canonical-cloud-web` must contain:

- `DATABASE_URL` for the exact non-owner `canonical_web_server` login;
- `APP_SESSION_ENCRYPTION_KEY` (standard-base64 encoded 32-byte key);
- `SUPABASE_URL` and `SUPABASE_PUBLISHABLE_KEY`;
- exact HTTPS origins in `APP_BASE_URL` and `APP_ALLOWED_ORIGINS` when those
  values are secret-store managed rather than fixed in the Deployment.

The API AWS object `dd/remote-dev/canonical-cloud-api` must contain:

- `DATABASE_URL` for the least-privilege API runtime login;
- `APP_SESSION_ENCRYPTION_KEY` shared with the browser session verifier;
- `SUPABASE_URL` and `SUPABASE_PUBLISHABLE_KEY`;
- `GEMINI_API_KEY` for quote analysis.

Only the API secret receives `GEMINI_API_KEY`. The browser pod and revoker must
not receive Gemini credentials. The model is selected independently through
`GEMINI_MODEL`; deployment certification must verify the configured model is
available to the provisioned Google project before traffic is enabled.

The quote AWS object `dd/remote-dev/canonical-cloud-quote` must contain:

- `DATABASE_URL` for the quote API's Canonical Supabase/PostgreSQL database;
- `SUPABASE_URL` and a non-privileged `SUPABASE_PUBLISHABLE_KEY`;
- `GEMINI_API_KEY`;
- one random `CANONICAL_WEB_SERVICE_TOKEN` shared only by the web and quote API.

The worker AWS object `dd/remote-dev/canonical-cloud-revoker` must contain:

- `SESSION_REVOCATION_DATABASE_URL` for the isolated
  `canonical_session_revoker` login;
- the same `APP_SESSION_ENCRYPTION_KEY` used by the application;
- `SUPABASE_URL` and `SUPABASE_PUBLISHABLE_KEY`.

The registry AWS object `dd/remote-dev/canonical-cloud-ghcr-pull` must contain a
`dockerconfigjson` property with a least-privilege machine-owned GHCR pull
credential. If every package is deliberately public, remove
`imagePullSecrets` and this ExternalSecret in a reviewed change rather than
storing a needless token.

Neither long-lived HTTP process nor the revoker receives a migration database
URL or Supabase service-role key. Schema and role bootstraps use a separate,
short-lived privileged identity outside Argo CD.

## Shared authentication

The Canonical Worker exposes the existing shared-auth customer deployment under
`/shared-auth/` on `app.canonical.plus`. The browser ceremony issues
Canonical host-only `__Host-` cookies, then returns only to a validated relative
path such as `/u/quote`. The shared-auth binary is reused across products, but
production project/database authority remains one reviewed deployment per realm
and Supabase project; Canonical does not receive Fiducia or OreSoftware cookies,
tables, or provider credentials.

Each Cloudflare route uses a dedicated HTTPS backend origin. WebSocket proxy
checks must prove that the `Upgrade` and `Connection` headers reach the selected
web or API Service without falling back to the public Worker hostname.

## Promotion

The checked-in `e09bb95160aaf95a836e810eb20b65e74f6317a6` tags are a coherent
release-SHA placeholder. The standalone API package does not exist for that
historical release, so this branch is intentionally non-activatable until the
monorepo publishes a new three-image release. After that release succeeds,
copy all three reported `sha256:` digests and run:

```sh
node remote/argocd/canonical-cloud/promote-release.mjs \
  --release-sha <40-lowercase-hex-monorepo-sha> \
  --web-digest sha256:<64-lowercase-hex> \
  --api-digest sha256:<64-lowercase-hex> \
  --revoker-digest sha256:<64-lowercase-hex>
```

Review the three deployment changes, run the contract/render tests, and commit
the digest promotion. The helper changes only the three image references and
six release annotations. Re-run it with the same values plus `--check` to
verify the committed state without writing.

The narrowly scoped promotion automation and secondary-submodule cleanup are
tracked in
[`docs/canonical-cloud-deployment-todos.md`](../../../docs/canonical-cloud-deployment-todos.md).

## Activation gates

Do not apply `remote/argocd/apps/canonical-cloud.application.yaml` until every
gate below is satisfied:

1. CI has passed for the exact monorepo web/revoker SHA and the exact API SHA,
   all GHCR images and attestations exist, and this overlay is pinned to their
   reported digests.
2. Every AWS Secrets Manager object above exists and External Secrets can
   materialize the web, API, revoker, and registry Kubernetes Secrets.
3. A human has reviewed and applied the schema migration plus the separate
   runtime and revoker role bootstraps. Migrations are never an Argo sync hook,
   init container, GitHub Actions step, or server-startup side effect.
4. DNS-only `canonical-web-origin.canonical.plus` and
   `canonical-api-origin.canonical.plus` records resolve to the ingress, and
   cert-manager has issued `canonical-cloud-origin-tls`. Only then may
   `app.canonical.plus` and `api.canonical.plus` be proxied through the Worker.
   The Worker must preserve cookies and WebSocket upgrade headers while stripping
   all caller-supplied `x-canonical-edge-*`, `x-auth-*`, and internal
   `x-canonical-service-*` / user-identity headers.
5. The new Service has been exercised directly in-cluster for `/healthz`,
   `/readyz`, REST authentication, session cookies, and WebSocket reconnects.

The Application manifest is intentionally dormant until an operator performs
its one-time installation. Once installed, automated prune and self-heal are
1. CI has passed for the exact monorepo SHA; all three GHCR images, SBOMs, and
   provenance attestations exist; and this overlay is pinned to those exact
   digests.
2. The Canonical Shared Auth server overlay and its first-party
   `/shared-auth/*` Ingress are healthy. Issuer, audience, Supabase project,
   signing/sealing keys, Redis namespace, and cookies must be Canonical-specific.
3. The four AWS Secrets Manager objects above exist and External Secrets can
   materialize each isolated Kubernetes Secret.
4. A human has reviewed and applied the PostgreSQL schema plus separate runtime
   and revoker role bootstraps. Migrations are never an Argo sync hook, init
   container, GitHub Actions deployment step, or server-startup side effect.
5. A dedicated HTTPS backend origin, proxied Cloudflare DNS records, valid
   certificates, and the three exact Worker routes exist. The origin must
   preserve Authorization, cookies, and WebSocket Upgrade/Connection headers.
6. In-cluster checks pass for both Services: `/healthz`, `/readyz`, Shared Auth
   verification, owner isolation, REST recovery, CSRF rejection, revocation,
   quote submission, Gemini failure handling, and WebSocket reconnect/status.
7. Anonymous `app.canonical.plus/u/quote` redirects to the first-party sign-in
   ceremony, while anonymous `api.canonical.plus/api/v1/quotes` returns a JSON
   `401` rather than a browser redirect.

The Application manifest remains dormant until an operator performs its
one-time installation. Once installed, automated prune and self-heal are
enabled for this dedicated path. Do not add this Application to a parent app
before the gates pass.

## Cutover and rollback

Route browser traffic to `canonical-cloud-web.canonical-cloud.svc.cluster.local:8081`
and API traffic to `canonical-api-server.canonical-cloud.svc.cluster.local:8081`
only after the activation checks pass. Keep the legacy `dd-canonical-cloud`
Deployment available during cutover; removing it is a separate reviewed change.

Rollback is a Git revert of the digest promotion or routing commit. Argo CD
reconciles the reverted desired state. Remove Cloudflare Worker routes before
changing DNS during an edge rollback, and never route around origin-side Shared
Auth verification. Do not use an imperative image change or an unreviewed
migration rollback, because either would make Git cease to be the auditable
source of truth.
