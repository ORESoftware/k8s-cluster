<!-- BEGIN k8s-cluster-submodule-notice -->
> [!NOTE]
> **Canonical source.** This repository is the source of truth for its code. It
> is also vendored as a **secondary** git submodule of
> [ORESoftware/k8s-cluster](https://github.com/ORESoftware/k8s-cluster) at
> `remote/deployments/3fa-web-server-rs` — make changes here, not in that
> submodule checkout.
>
> On disk: source clone `~/codes/3FA-app/3fa-web-server.rs` · submodule checkout
> `~/codes/ores/k8s-cluster/remote/deployments/3fa-web-server-rs`.
<!-- END k8s-cluster-submodule-notice -->

# 3fa-web-server.rs — 3FA

**[3FA-app](https://github.com/3FA-app)** · site: [3fa-app.github.io](https://3fa-app.github.io)

Web version of the **3FA authenticator** — shared-auth sessions backed by the
3FA Supabase provider, plus a TOTP enrollment demo (QR provisioning + 6-digit
code verification), rendered server-side.

Sibling repos:

- [3fa-backend.rs](https://github.com/3FA-app/3fa-backend.rs) — zero-knowledge sync server
- [3FA-desktop.rs](https://github.com/3FA-app/3FA-desktop.rs) — desktop authenticator app

> **Trust boundary:** shared-auth login gates only *this web session*. The sync
> vault handled by `3fa-backend.rs` stays zero-knowledge — the server never
> sees vault plaintext or the keys to it, and nothing in this repo changes that.

## Stack — MASH

**m**aud (HTML) · **a**xum (HTTP) · **S**eaORM over Postgres/Supabase · **h**tmx (interactivity).

> **ORM policy:** prefer **SeaORM** over sqlx for new database code. This v1 is
> deliberately **database-less**: sessions are short-lived shared-auth access
> tokens in HttpOnly cookies, and enrollment secrets live in an HMAC-signed
> cookie. When
> persistence lands (enrolled devices, per-user TOTP secrets, JWT verification
> against a user table), reach for SeaORM entities on Supabase Postgres first.

## Routes

- `GET /`, `GET /login` — login page (email + password, htmx post)
- `POST /login` — server-side Supabase password grant followed immediately by a
  shared-auth provider exchange; sets only the shared-auth access token in the
  `threefa_session` cookie (HttpOnly, SameSite=Lax) and redirects to `/enroll`
- `GET /enroll` — TOTP enrollment: fresh 20-byte secret, inline SVG QR of the
  `otpauth://` URI (no external requests), base32 fallback, code form.
  Requires a session that passes shared-auth `/auth/verify`.
- `POST /enroll/verify` — RFC 6238 verification (SHA-1, 30 s step, ±1 step skew)
- `GET /livez`, `GET /healthz` — process liveness (`healthz` is the compatibility alias)
- `GET /readyz` — traffic readiness
- `GET /metrics` — bounded Prometheus request metrics

TOTP + base32 are implemented by hand on `hmac` + `sha1` (no `totp-rs`); the
RFC 6238 test vectors run in `cargo test`.

## Environment

| Variable            | Required | Description                                                        |
| ------------------- | -------- | ------------------------------------------------------------------ |
| `SUPABASE_URL`      | for login | Supabase project URL, e.g. `https://xyz.supabase.co`              |
| `SUPABASE_ANON_KEY` | for login | Supabase anon (public) API key                                    |
| `SHARED_AUTH_BASE_URL` | for login | shared-auth service/gateway base URL                           |
| `SERVER_SECRET`     | no       | HMAC key for the enrollment cookie; random at boot if unset        |
| `PORT`              | no       | Listen port, default `8080`                                        |
| `BIND_ADDR`         | no       | Full listen address; overrides `PORT`                              |
| `RUST_LOG`          | no       | Structured-log filter, default `info`                              |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | no | OTLP/HTTP collector base URL                                 |

Without both provider settings and `SHARED_AUTH_BASE_URL`, the site still
renders with a notice and `POST /login` answers `503`.

## Run

```bash
cargo run            # binds 0.0.0.0:8080 (override with PORT)
cargo test           # TOTP vectors, base32, otpauth URI, route tests
```

## Observability

- Structured JSON records go to stdout for Promtail/Loki and contain the active
  OTEL `trace_id`/`span_id`; passwords, access tokens, TOTP secrets, and email
  addresses are never logged.
- Provider login, shared-auth exchange, session verification, and enrollment
  emit low-cardinality auth outcome events inside the request trace; outbound
  auth calls propagate W3C trace context.
- Incoming W3C `traceparent` is preserved and server spans export over
  OTLP/HTTP to the in-cluster collector.
- `/metrics` exposes low-cardinality request totals, latency, and in-flight
  counts for Prometheus scraping.

## Layout

```text
src/main.rs        Minimal binary entrypoint
src/server.rs      Listener lifecycle and graceful SIGTERM shutdown
src/config.rs      Environment/provider/shared-auth configuration
src/state.rs       Shared HTTP client, secret, and metrics
src/app.rs         Routes, middleware, and route integration tests
src/login.rs       Provider login followed by shared-auth exchange
src/shared_auth.rs Central session exchange/verification client
src/enrollment.rs  TOTP enrollment flow
src/cookies.rs     Signed-cookie helpers
src/views.rs       Maud page rendering and styles
src/totp.rs        RFC 4648/6238 primitives
src/telemetry.rs   OTLP traces and Loki-compatible JSON logs
src/metrics.rs     Prometheus metrics
```

## Deployment boundary

Develop and validate in this standalone clone. After the canonical change is
merged, bump `remote/deployments/3fa-web-server-rs` in `k8s-cluster`; never edit
the secondary checkout directly. The Kubernetes manifest builds/runs from that
submodule path and exposes OTLP, Prometheus, and health configuration explicitly.

## Cross-surface delivery

A user-visible, contract, authentication, enrollment, recovery, notification,
permission, navigation, or deep-link change in this Rust web server must also be
evaluated for:

- the current Flutter mobile/mobile-web implementation
  [`ORESoftware/3fa-client-ui.dart`](https://github.com/ORESoftware/3fa-client-ui.dart)
  and its planned organization-local target `3FA-app/3fa-flutter`;
- the Flutter desktop targets in that same Flutter application;
- the native Rust desktop app
  [`3FA-app/3FA-desktop.rs`](https://github.com/3FA-app/3FA-desktop.rs);
- `3FA-app` shared interfaces, clients, route types, and conformance fixtures.

This is a judgment call, not a requirement to duplicate every server-rendered
page. SEO, public-web presentation, and server operations may remain web-only.
Changes to authentication semantics, TOTP/HOTP behavior, vault or device state,
validation, errors, or user navigation normally require coordinated client work.
Every issue and pull request must record which surfaces were evaluated, which
change now, why any surface does not change, and any accepted parity gap.

Deep links are HTTPS-first and share one versioned route model across web,
Android/iOS, Flutter Web, Flutter desktop, and Rust desktop:

```text
https://<verified-3fa-owned-host>/open/<route>?<bounded-query>
```

Fallback navigation uses `threefa://`. The production HTTPS host must not be
guessed. Both app implementations must support cold start, already-running
single-instance delivery, authentication resume, and browser fallback. TOTP
seeds, recovery secrets, vault material, bearer/refresh tokens, credentials,
and private account data are prohibited in URLs; sensitive handoffs use
short-lived, single-use, audience-bound codes.

See [`docs/CROSS_SURFACE_DELIVERY.md`](docs/CROSS_SURFACE_DELIVERY.md) and the
[portfolio policy](https://github.com/ORESoftware/project-registry/blob/main/docs/cross-surface-delivery.md).
