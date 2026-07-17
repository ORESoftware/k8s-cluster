<!-- BEGIN k8s-cluster-submodule-notice -->
> [!NOTE]
> **Canonical source.** This repository is the source of truth for its code. It
> is also vendored as a **secondary** git submodule of
> [ORESoftware/k8s-cluster](https://github.com/ORESoftware/k8s-cluster) at
> `remote/deployments/3fa-web-server-rs` — make changes here, not in that
> submodule checkout.
<!-- END k8s-cluster-submodule-notice -->

# 3fa-web-server.rs — 3FA

**[3FA-app](https://github.com/3FA-app)** · site: [3fa-app.github.io](https://3fa-app.github.io)

Web version of the **3FA authenticator** — Supabase-backed sign-in and a TOTP
enrollment demo (QR provisioning + 6-digit code verification), rendered
server-side.

Sibling repos:

- [3fa-backend.rs](https://github.com/3FA-app/3fa-backend.rs) — zero-knowledge sync server
- [3FA-desktop.rs](https://github.com/3FA-app/3FA-desktop.rs) — desktop authenticator app

> **Trust boundary:** Supabase login gates only *this web session*. The sync
> vault handled by `3fa-backend.rs` stays zero-knowledge — the server never
> sees vault plaintext or the keys to it, and nothing in this repo changes that.

## Stack — MASH

**m**aud (HTML) · **a**xum (HTTP) · **S**eaORM over Postgres/Supabase · **h**tmx (interactivity).

> **ORM policy:** prefer **SeaORM** over sqlx for new database code. This v1 is
> deliberately **database-less**: sessions are cookie-borne Supabase access
> tokens, and enrollment secrets live in an HMAC-signed cookie. When
> persistence lands (enrolled devices, per-user TOTP secrets, JWT verification
> against a user table), reach for SeaORM entities on Supabase Postgres first.

## Routes

- `GET /`, `GET /login` — login page (email + password, htmx post)
- `POST /login` — server-side Supabase password grant; sets the
  `threefa_session` cookie (HttpOnly, SameSite=Lax) and redirects to `/enroll`
- `GET /enroll` — TOTP enrollment: fresh 20-byte secret, inline SVG QR of the
  `otpauth://` URI (no external requests), base32 fallback, code form.
  Requires the session cookie; full JWT verification is a documented TODO.
- `POST /enroll/verify` — RFC 6238 verification (SHA-1, 30 s step, ±1 step skew)
- `GET /healthz` — liveness

TOTP + base32 are implemented by hand on `hmac` + `sha1` (no `totp-rs`); the
RFC 6238 test vectors run in `cargo test`.

## Environment

| Variable            | Required | Description                                                        |
| ------------------- | -------- | ------------------------------------------------------------------ |
| `SUPABASE_URL`      | for login | Supabase project URL, e.g. `https://xyz.supabase.co`              |
| `SUPABASE_ANON_KEY` | for login | Supabase anon (public) API key                                    |
| `SERVER_SECRET`     | no       | HMAC key for the enrollment cookie; random at boot if unset        |
| `PORT`              | no       | Listen port, default `8080`                                        |

Without the Supabase vars the site still renders (with a notice) and `POST
/login` answers `503 Supabase not configured`.

## Run

```bash
cargo run            # binds 0.0.0.0:8080 (override with PORT)
cargo test           # TOTP vectors, base32, otpauth URI, route tests
```
