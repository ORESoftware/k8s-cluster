# Fiducia.cloud Security Audit

**Original review:** 2026-07-08

**Remediation revalidation:** 2026-07-13

**Scope:** All app repositories pinned under `fiducia-monorepo` (auth, admin, backend,
brain, load-balance, node, node-sidecar, edge, telemetry, infra, UI/web) plus the
GitOps superproject itself.

**Method:** Static review of authentication/authorization code, secret handling,
container images, CI workflow posture, and repository visibility. Line references
were verified against the tree at audit time and may drift as code changes.

The detailed findings below preserve the original evidence. The summary table
and final remediation section record the current state after the 2026-07-13
fleet-wide hardening pass; do not treat historical code snippets as the live
implementation.

> **Companion:** open cross-repo *architectural* findings (shared-secret identity,
> web-tier login rate-limiting, infra hardening, and the 2026-07-14 messaging/node
> code findings) are tracked separately in
> [`architectural-security-findings.md`](./architectural-security-findings.md).

The codebase is, on the whole, **well-hardened**: JWT algorithm-confusion is
actively defended, secret comparisons are constant-time, admin HTML output is
consistently escaped, the admin dev-session bypass is fail-closed in release
builds, and no real secrets are committed. The findings below are mostly
defense-in-depth gaps and deployment-posture footguns rather than directly
exploitable holes — with one exception (#1) that becomes exploitable if a single
Kubernetes Secret is never provisioned.

---

## Summary (by severity)

| # | Severity | Repo · File | Issue | One-line fix | Status |
|---|----------|-------------|-------|--------------|--------|
| 0 | **High** (owner decision) | `docs/repo-boundaries.md` vs GitHub | 12 private-by-default repos, including the monorepo, are currently **public** | Review real visibility against intent | Flagged; live snapshot documented, settings unchanged |
| 1 | **Med-High** | `fiducia-node.rs` + infra | Trusted-hop guard and secret wiring were fail-open | Require the secret and fail closed | **RESOLVED** |
| 2 | **Med** | `fiducia-admin.rs` | Session cookie lacked `Secure` | Enforce `Secure` in release builds | **RESOLVED** |
| 3 | **Med** | `fiducia-auth.rs` | API-key pepper was not applied | HMAC-SHA256(secret, pepper) with controlled legacy reads | **RESOLVED** |
| 4 | **Med** | `fiducia-edge/Dockerfile` | root user, `npm install`, ships `wrangler dev --ip 0.0.0.0` as runtime | Lockfile install, non-root, production CMD | **APPLIED** |
| 5 | **Low-Med** | all `.github/workflows/*` | Mutable actions and fail-open checks | Exact action/tool pins and blocking gates | **RESOLVED** |
| 6 | **Low** | fleet Dockerfiles | Root/mutable images and moving sibling inputs | Non-root runtimes, exact sibling SHAs, digest-pinned bases | **RESOLVED** |
| 7 | **Low** | `fiducia-auth.rs/src/supabase.rs` | Hardcoded production project fallback | Require explicit Supabase configuration | **RESOLVED** |
| 8 | **Low** | `fiducia-admin.rs` | Broad all-users promotion switch | Remove the unsafe authority shortcut | **RESOLVED** |

---

## What's done right (keep it this way)

These are load-bearing controls that the audit confirms are implemented
correctly. Call them out in review so they are not regressed.

- **JWT algorithm-confusion defense (two independent verifiers).**
  - `fiducia-load-balance.rs/src/auth.rs` (~213): rejects any token whose header
    algorithm is not asymmetric (`is_asymmetric_algorithm`), *requires* a `kid`,
    resolves the JWK by `kid`, and refuses symmetric (`OctetKey`) JWKs (~505).
  - `fiducia-auth.rs/src/supabase.rs` (~54, ~288): same asymmetric-only + `kid`
    gate before offline JWKS verification, with `Validation` pinning issuer and
    audience (~90). A regression test asserts `HS256` is not accepted for offline
    JWKS (`symmetric_jwt_algorithms_are_not_accepted_for_offline_jwks`).
  - This blocks the classic "sign with HS256 using the RSA public key as the HMAC
    secret" attack.
- **Constant-time secret comparison.** `fiducia-auth.rs/src/keys.rs` (~240) and
  `fiducia-node.rs/src/internal_auth.rs` (~101) both use length-then-content
  compares that do not short-circuit, so API keys / internal secrets cannot be
  recovered a byte at a time via timing.
- **Consistent XSS escaping in admin.** `fiducia-admin.rs/src/views.rs` funnels
  all interpolated values through a single `esc()` helper (used ~12×) with a
  `esc_neutralizes_markup` test.
- **Fail-closed admin dev-session.** `fiducia-admin.rs/src/session.rs` ignores the
  `FIDUCIA_ADMIN_DEV_SESSION` bypass in release builds unless
  `FIDUCIA_ALLOW_INSECURE_DEV_SESSION=1` is explicitly set — "production can't
  silently hand out admin."
- **Clean secret hygiene.** No real secrets in tree; `.env.example` uses
  `replace-me…` / `…secret-manager-value` placeholders; real values are expected
  in gitignored files or a secret manager (per `docs/repo-boundaries.md`).
- **CSPRNG key material.** API-key halves are 256-bit OS-CSPRNG (`getrandom`),
  never storing the raw secret.

---

## Findings

### #0 — Visibility mismatch (High, flag-only)

**Where:** `fiducia-monorepo/docs/repo-boundaries.md` (§ Visibility Defaults,
lines ~22–35) vs. actual GitHub visibility.

**Issue:** The boundaries doc marks the following as **private by default**, but
they were all **PUBLIC** in the 2026-07-13 live GitHub audit:

| Repo | Documented | Actual (GitHub) |
|------|-----------|-----------------|
| `fiducia-monorepo` | private | **public** |
| `fiducia-infra` | private | **public** |
| `fiducia-auth.rs` | private | **public** |
| `fiducia-admin.rs` | private | **public** |
| `fiducia-customer.rs` | private | **public** |
| `fiducia-brain.rs` | private | **public** |
| `fiducia-load-balance.rs` | private | **public** |
| `fiducia-node.rs` | private | **public** |
| `fiducia-node-sidecar.rs` | private | **public** |
| `fiducia-edge` | private | **public** |
| `fiducia-telemetry.rs` | private | **public** |
| `fiducia-customer-ui.web` | private | **public** |

This is documentation-vs-reality drift on *security-sensitive control-plane,
auth, and customer surfaces*. Public exposure of these repos means the trust
model in this very audit (e.g. #1's "the node port is unreachable from outside
the cluster") is being reasoned about against source that any attacker can read,
and any *future* private commit to one of them would need the repo flipped first.

**Fix (one line):** Reconcile each repo's real visibility with intent — either
update the doc to reflect an intentional open-source posture, or flip the repos
back to private. **Do not change visibility as part of this audit;** this is a
flag for an owner decision, because flipping a public repo private (or vice
versa) has release, fork, and disclosure consequences.

---

### #1 — Fail-open internal trust boundary (Med-High)

**Where:** `fiducia-node.rs/src/internal_auth.rs` (~90, `authorized`). Wiring:
`fiducia-infra/base/node/statefulset.yaml:95`,
`fiducia-infra/base/brain/statefulset.yaml:77`,
`fiducia-infra/base/load-balance/deployment.yaml:72`.

**Issue:** The node has no per-request user authz of its own — it trusts the
`x-fiducia-org-id` the load balancer injects. The `internal_auth` middleware is
meant to gate `/v1` and `/raft` to trusted hops via the shared
`FIDUCIA_INTERNAL_SECRET`. But the guard is **fail-open**:

```rust
pub fn authorized(expected: Option<&str>, provided: Option<&str>) -> bool {
    match expected {
        None => true,                 // <-- secret unset => allow ANY caller
        Some(secret) => provided.map(|p| constant_time_eq(...)).unwrap_or(false),
    }
}
```

`None` means "guard disabled → allow everything." Meanwhile **all three** infra
manifests wire the secret with `secretKeyRef: { … optional: true }`, and the
knob is **absent from `.env.example`**. So if the `fiducia-secrets` Secret is
never provisioned (the default until an operator runs the documented
`kubectl create secret …`), every node comes up with the guard *off* — and
anything that can reach the node port can forge `x-fiducia-org-id` to act as any
org, or forge Raft `AppendEntries` to corrupt a shard log. The startup log does
warn, but nothing *enforces*.

The fail-open default is a deliberate choice (single-node/dev and loopback tests
stay byte-identical), which is fine for dev — the problem is that **prod inherits
the same default** with no floor.

**Recommended patch (documented — do NOT apply here; touches trust logic):**
Make the posture fail-closed **only in production**, keyed off the existing
`FIDUCIA_DEPLOYMENT_ENV` knob (already read by `fiducia-telemetry.rs/src/lib.rs`):

```rust
// in init_and_log() / startup, before serving:
let is_prod = std::env::var("FIDUCIA_DEPLOYMENT_ENV")
    .map(|v| v.eq_ignore_ascii_case("prod") || v.eq_ignore_ascii_case("production"))
    .unwrap_or(false);
if is_prod && configured().is_none() {
    // refuse to start rather than serve /v1 + /raft open to the cluster
    panic!("FIDUCIA_INTERNAL_SECRET is required when FIDUCIA_DEPLOYMENT_ENV=prod");
}
```

Then in infra, flip the three `secretKeyRef` entries to `optional: false` so the
pod won't schedule without the Secret, and add
`FIDUCIA_INTERNAL_SECRET=replace-me-with-secret-manager` to `.env.example`.
(Longer term this is the cheap complement to a real mTLS / NetworkPolicy posture,
per future-work #1.)

> Note: this audit did **not** add the knob to any service `.env.example` because
> none of `fiducia-node.rs` / `fiducia-brain.rs` / `fiducia-load-balance.rs`
> currently ships one — the only `.env.example` in the tree is
> `fiducia-monorepo/.env.example`. Adding the knob there (or creating per-service
> examples) is part of this recommendation, left for the owner.

---

### #2 — Session cookie missing `Secure` (Med)

**Where:** `fiducia-admin.rs/src/main.rs` (~141, `login_submit`).

**Issue:** The admin session cookie is set as:

```rust
let cookie = format!("fiducia_session={token}; Path=/; HttpOnly; SameSite=Lax");
```

`HttpOnly` and `SameSite=Lax` are present, but there is **no `Secure`
attribute** — the session token may be sent over plaintext HTTP (e.g. a
misconfigured ingress, a same-site downgrade, or local proxying), where it can be
captured.

**Fix (one line):** Append `; Secure` to the cookie string (guard it behind
"not localhost/dev" if HTTP is required for local testing).

*Doc only — `fiducia-admin.rs` is owned elsewhere; not modified in this pass.*

---

### #3 — API-key pepper required but never applied (Med)

**Where:** `fiducia-auth.rs/src/keys.rs` (~235, `hash_secret`). Config:
`fiducia-monorepo/.env.example:32` (`CUSTOMER_API_KEY_PEPPER`) and `:33`
(`CUSTOMER_API_KEY_HASH_ALGORITHM=sha256`).

**Issue:** `CUSTOMER_API_KEY_PEPPER` is a first-class, secret-manager-sourced
config value (and, per the task, test-enforced as required), but the actual hash
is a **plain, unkeyed SHA-256** of the secret:

```rust
fn hash_secret(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());   // pepper never mixed in
    format!("sha256:{}", to_hex(&digest))
}
```

The pepper is loaded and demanded but silently unused. If the key-hash store
leaks, an attacker can brute/rainbow the stored digests without needing the
pepper at all — defeating the entire reason a pepper exists (a server-side secret
that must also be stolen). API-key secrets are 256-bit CSPRNG, so preimage brute
force is impractical *today*; the pepper is the defense-in-depth that is being
paid for but not received.

**Recommended patch (documented — do NOT apply here; touches key/trust logic):**
Key the digest with the pepper via HMAC, and keep the algorithm tag honest:

```rust
use hmac::{Hmac, Mac};
use sha2::Sha256;

fn hash_secret(secret: &str) -> String {
    let pepper = std::env::var("CUSTOMER_API_KEY_PEPPER")
        .expect("CUSTOMER_API_KEY_PEPPER is required");
    let mut mac = Hmac::<Sha256>::new_from_slice(pepper.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(secret.as_bytes());
    format!("hmac-sha256:{}", to_hex(&mac.finalize().into_bytes()))
}
```

Note this changes the stored digest format, so it needs a migration / dual-read
window (accept both `sha256:` and `hmac-sha256:` prefixes during rollout).

---

### #4 — Edge Dockerfile: root, `npm install`, dev server as runtime (Med) — **APPLIED**

**Where:** `fiducia-edge/Dockerfile`.

**Issue (before):** ran as **root**, used non-reproducible `npm install` (no
lockfile), and shipped the Cloudflare **dev server bound to all interfaces** as
the container's runtime command:

```dockerfile
COPY package.json wrangler.toml ./
RUN npm install && npm run check
EXPOSE 8787
CMD ["npx", "wrangler", "dev", "--ip", "0.0.0.0"]
```

`wrangler dev --ip 0.0.0.0` is a development server (hot-reload, permissive
inspector, no production hardening) exposed on every interface — inappropriate as
a shipped runtime.

**Fix:** APPLIED — see [Applied fixes](#applied-fixes-this-pass). Lockfile-pinned
`npm ci`, non-root `USER node`, and a production CMD (`npm run deploy`, i.e.
`wrangler deploy`) that publishes the Worker to Cloudflare's edge instead of
serving a dev server. Verified: image builds; `docker inspect` shows
`User=node`, `Cmd=["npm","run","deploy"]`; wrangler resolves at runtime.

---

### #5 — Workflow actions pinned to mutable major tags (Low-Med)

**Where:** every `*/.github/workflows/*.yml` (e.g. `actions/checkout@v4`,
`docker/login-action@v3`, `docker/build-push-action@v6`, `Swatinem/rust-cache@v2`,
`dtolnay/rust-toolchain@stable`).

**Issue:** Third-party actions are referenced by **mutable** major tags (or
`@stable`). Whoever controls that tag can move it to new code, which then runs in
CI with repository/registry credentials — a supply-chain vector (cf. the
`tj-actions/changed-files` compromise class).

**Fix (one line):** Pin actions to immutable **commit SHAs** (with a comment
noting the version), and let Dependabot bump them.

*Doc only — workflow files are owned elsewhere and were not modified. Per the
task, Dependabot is being added, which addresses the update cadence.*

---

### #6 — Root-user images + unpinned interfaces clone (Low)

**Where:** `fiducia-telemetry.rs/Dockerfile`, `fiducia-infra/Dockerfile`,
`fiducia-marketing.web/Dockerfile`.

**Issue:**
- **`fiducia-telemetry.rs/Dockerfile`** ran the test image as **root** and cloned
  the interfaces dependency unpinned (`ARG INTERFACES_REF=main` →
  `git clone --branch main …`), so a build is not reproducible and picks up
  whatever `main` points at.
- **`fiducia-infra/Dockerfile`** runs as **root** and uses `npm install` (not
  `npm ci`).
- **`fiducia-marketing.web/Dockerfile`** already uses `npm ci` (good) but the final
  `nginx` stage runs as **root** (no `USER`).

**Fix:**
- Telemetry non-root: **APPLIED** — see [Applied fixes](#applied-fixes-this-pass).
- Pin `INTERFACES_REF` to a tag/SHA instead of `main` (doc — build-input change).
- `fiducia-infra`: add a non-root `USER` and switch to `npm ci` (doc).
- `fiducia-marketing.web`: add a non-root `USER` in the nginx stage, e.g. run as
  `nginx-unprivileged` / bind an unprivileged port (doc).

---

### #7 — Hardcoded Supabase project-ref fallback (Low)

**Where:** `fiducia-auth.rs/src/supabase.rs:26`.

**Issue:** `const DEFAULT_PROJECT_REF: &str = "ruxctrzdvugxztbjcpoi";` — a real
project ref baked into source as a fallback. It leaks the production project
identifier into (now public, per #0) source, and a misconfigured deployment that
forgets `SUPABASE_PROJECT_REF` silently verifies tokens against this default
project rather than failing.

**Fix (one line):** Drop the hardcoded default; require `SUPABASE_PROJECT_REF`
(or its derived issuer/JWKS URL) from config and fail fast if absent.

*Doc only.*

---

### #8 — `FIDUCIA_ADMIN_ALL_USERS` promotes everyone to admin (Low)

**Where:** `fiducia-admin.rs/src/session.rs` (~155, `admin_all_users`).

**Issue:** When `FIDUCIA_ADMIN_ALL_USERS` is set to a truthy value
(`1/true/yes/on/…`), **every** authenticated user is treated as an admin. This is
a convenience/dev switch, but unlike the dev-session bypass (#done-right, which is
compile-gated to non-release builds) this one is a **plain runtime env check with
no prod guard** — a single stray environment variable in a prod manifest silently
grants org-wide admin.

**Fix (one line):** Gate it behind the same non-release / explicit-insecure-opt-in
mechanism as the dev-session bypass, and log loudly when it is active.

*Doc only — `fiducia-admin.rs` is owned elsewhere; not modified in this pass.*

---

## Applied fixes (this pass)

Only low-risk, mechanical container hardening was applied. No Rust logic, no
`fiducia-admin.rs`, and no `.github/workflows/*` files were touched. Nothing was
committed or pushed.

### `fiducia-edge/Dockerfile` (finding #4)

**Before:**
```dockerfile
# syntax=docker/dockerfile:1
# Cloudflare Worker tooling image.
FROM node:24-slim
WORKDIR /app
COPY package.json wrangler.toml ./
COPY src src
RUN npm install && npm run check
EXPOSE 8787
CMD ["npx", "wrangler", "dev", "--ip", "0.0.0.0"]
```

**After:**
```dockerfile
# syntax=docker/dockerfile:1
# Cloudflare Worker tooling / deploy image for fiducia-edge.
FROM node:24-slim
WORKDIR /app
COPY package.json package-lock.json wrangler.toml ./
COPY src src
# Deterministic, lockfile-pinned install; syntax-check the Worker entrypoint.
RUN npm ci && npm run check
# Drop root — wrangler needs no privileges. Give the unprivileged `node` user
# (present in the official image) ownership so `wrangler deploy` can write its
# build output under /app/.wrangler at run time.
RUN chown -R node:node /app
USER node
# The Worker runs on Cloudflare's edge, not inside this container, so there is no
# long-running server to expose. The production entrypoint publishes the Worker
# (needs CLOUDFLARE_API_TOKEN / CLOUDFLARE_ACCOUNT_ID at run time) instead of
# shipping the insecure `wrangler dev --ip 0.0.0.0` dev server.
CMD ["npm", "run", "deploy"]
```

Notes: `package-lock.json` exists and is in sync, so `npm ci` is safe; the
`file:../fiducia-interfaces` dependency resolves to a (harmless) dangling symlink
inside the container exactly as it did under `npm install`, and `npm run check`
(`node --check`) only parses `src/index.mjs` so it does not need that dependency.
`EXPOSE 8787` was dropped because the container no longer serves a port —
`package.json` exposes a `deploy` script (`wrangler deploy`) which is a one-shot
publish, so it is used as the production CMD per the "use the deploy script if one
exists" guidance. **Verified with a real `docker build` + `docker inspect`.**

### `fiducia-telemetry.rs/Dockerfile` (finding #6)

**Before:**
```dockerfile
...
WORKDIR /build/fiducia-telemetry.rs
RUN cargo test
CMD ["cargo", "test"]
```

**After:**
```dockerfile
...
WORKDIR /build/fiducia-telemetry.rs
RUN cargo test
# Run the container as an unprivileged user instead of root. Create the user and
# hand it the build tree + cargo caches so `cargo test` can still write target/
# and the registry lock when the image is run.
RUN useradd --create-home --uid 10001 ci \
    && chown -R ci:ci /build "${CARGO_HOME:-/usr/local/cargo}"
USER ci
CMD ["cargo", "test"]
```

Notes: All build-time `RUN` steps stay as root (they precede `USER`), so the image
still builds; the final `USER ci` (uid 10001) means the container no longer runs
as root, and chowning `/build` + `CARGO_HOME` keeps the runtime `cargo test`
working non-root. Validated against the `rust:1-slim-bookworm` base that
`useradd` is present and `CARGO_HOME=/usr/local/cargo`. The unpinned
`INTERFACES_REF=main` clone (also part of #6) was left as a documented
recommendation, not changed.

### `.env.example` — not applied (scoped out by precondition)

The task's `.env.example` change was conditioned on the file existing in
`fiducia-node.rs`, `fiducia-brain.rs`, or `fiducia-load-balance.rs`. **None of
those three repos ships an `.env.example`** — the only one in the tree is
`fiducia-monorepo/.env.example`, which is outside the enumerated set. So no
`.env.example` was modified. Adding `FIDUCIA_INTERNAL_SECRET` remains part of the
#1 recommendation (owner to decide whether it lives in the monorepo example or in
new per-service examples).

---

## 2026-07-13 remediation status

The follow-through pass semantically applied findings #1 through #8 across the
owning repositories, added regression tests, updated their READMEs, validated
the focused suites, and pushed every component `main`. CI now blocks on action
pins, locked dependency resolution, audits, and browser/package gates. Every
Dockerfile uses an explicit non-root runtime and registry-verified
`tag@sha256` bases with Docker Dependabot coverage. Cross-repository build inputs
are full reviewed commit IDs rather than moving branches.

Finding #0 remains intentionally external: repository visibility was inspected
but not changed. The current snapshot and private-by-default intent now coexist
explicitly in `docs/repo-boundaries.md`; an owner must decide whether to restore
the twelve mismatched repositories to private or accept/document a public
release posture.
