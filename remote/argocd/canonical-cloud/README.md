# canonical.cloud backend GitOps

This overlay is the dedicated deployment boundary for the canonical.cloud Rust
backend. It is deliberately separate from `dd-next-runtime`: the legacy
`dd-canonical-cloud` workload remains untouched until this prebuilt-image path
has been activated and verified.

`canonical-cloud/canonical-monorepo` remains the deployable source of truth for
the web and revoker images. The dedicated quote API is released from
`canonical-cloud/canonical-api-server.rs` because that repository owns its
runtime, container contract, and immutable digest artifact. This overlay
consumes only reviewed immutable digests and never builds source in-cluster. The existing
`remote/deployments/canonical-cloud` secondary submodule is legacy operational
context and is not an input to this overlay.

Argo CD is the only runtime writer. GitHub Actions builds and attests the web
and revoker images, but it receives no kubeconfig and never applies Kubernetes
resources. A reviewed Git commit promotes exact registry digests here; Argo CD
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
- `canonical-cloud-revoker` runs `canonical-session-revoker run`. It declares no
  port, Service, Ingress, or accepted NetworkPolicy ingress.
- `canonical-cloud-web-runtime` and `canonical-cloud-revoker-runtime` are
  different Kubernetes Secrets backed by different AWS Secrets Manager
  objects. The web process receives `DATABASE_URL`; only the worker receives
  `SESSION_REVOCATION_DATABASE_URL`.
- `canonical-cloud-ghcr-pull` is a third, registry-only Secret. It must contain
  a least-privilege, machine-owned GHCR pull credential and must never reuse a
  runtime database or Supabase credential. If both packages are deliberately
  made public, remove `imagePullSecrets` and this ExternalSecret in a reviewed
  change rather than storing a needless token.
- Neither long-lived process receives a migration database URL or Supabase
  service-role key. Both use only the Supabase publishable key.

The web AWS object `dd/remote-dev/canonical-cloud-web` must contain:

- `DATABASE_URL` for the exact non-owner `canonical_web_server` login;
- `APP_SESSION_ENCRYPTION_KEY` (standard-base64 encoded 32-byte key);
- `SUPABASE_URL` and `SUPABASE_PUBLISHABLE_KEY`;
- exact HTTPS origins in `APP_BASE_URL` and `APP_ALLOWED_ORIGINS` (no wildcard).

The quote AWS object `dd/remote-dev/canonical-cloud-quote` must contain:

- `DATABASE_URL` for the quote API's Canonical Supabase/PostgreSQL database;
- `SUPABASE_URL` and a non-privileged `SUPABASE_PUBLISHABLE_KEY`;
- `GEMINI_API_KEY`;
- one random `CANONICAL_WEB_SERVICE_TOKEN` shared only by the web and quote API.

The worker AWS object `dd/remote-dev/canonical-cloud-revoker` must contain:

- `SESSION_REVOCATION_DATABASE_URL` for the isolated
  `canonical_session_revoker` login;
- the same `APP_SESSION_ENCRYPTION_KEY` used by the web process;
- `SUPABASE_URL` and `SUPABASE_PUBLISHABLE_KEY`.

The registry AWS object `dd/remote-dev/canonical-cloud-ghcr-pull` must contain a
`dockerconfigjson` property whose value is a valid Docker config JSON document
for `ghcr.io`. Do not commit the document or token.

## Shared authentication

The Canonical Worker exposes the existing shared-auth customer deployment under
`/shared-auth/` on `app.canonical.plus`. The browser ceremony issues
Canonical host-only `__Host-` cookies, then returns only to a validated relative
path such as `/u/quote`. The shared-auth binary is reused across products, but
production project/database authority remains one reviewed deployment per realm
and Supabase project; Canonical does not receive Fiducia or OreSoftware cookies,
tables, or provider credentials.

## Promotion

The checked-in `e245ed408810455b7a0c43b9f4e81fd60b172100` image tags are an
immutable release-SHA placeholder. Do not activate the Argo Application while
they are still tag references. After the matching release workflow succeeds,
copy the two reported `sha256:` digests and run:

```sh
node remote/argocd/canonical-cloud/promote-release.mjs \
  --release-sha e245ed408810455b7a0c43b9f4e81fd60b172100 \
  --web-digest sha256:<64-lowercase-hex> \
  --revoker-digest sha256:<64-lowercase-hex>
```

Review the two deployment changes, run the contract/render tests, and commit
the digest promotion. The helper changes only the two image references and
their release annotations. Re-run it with the same values plus `--check` to
verify the committed state without writing.

This reviewed manual handoff is temporary. The narrowly scoped promotion
automation and secondary-submodule cleanup are tracked in
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
enabled for this dedicated path. Do not add this Application to a parent app
before the gates pass.

## Cutover and rollback

Route traffic to `canonical-cloud-web.canonical-cloud.svc.cluster.local:8081`
only after activation checks pass. Keep the legacy `dd-canonical-cloud`
Deployment available during the cutover; removing it is a separate reviewed
change.

Rollback is a Git revert of the digest promotion or routing commit. Argo CD
reconciles the reverted desired state. Do not use an imperative image change or
an unreviewed migration rollback, because either would make Git cease to be the
auditable source of truth.
