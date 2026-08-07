# Cross-surface delivery

Verified **2026-08-06**.

## Surfaces

| Surface | Repository/status |
|---|---|
| Rust server/web authority | `shared-auth/shared-auth-server.rs` — current repository |
| Flutter mobile, Flutter Web, and Flutter desktop | `shared-auth/shared-auth-flutter` — planned |
| Rust desktop | `shared-auth/shared-auth-desktop.rs` — planned native Floem application |
| Shared contracts | Shared Auth interfaces, generated clients, policy types, route and OAuth/OIDC fixtures |

Planned repository names are allocations, not evidence that the remotes exist.

## Judgment-based propagation

Evaluate Android/iOS, Flutter Web, Flutter desktop, Floem desktop, and shared contracts for every user-visible or contract-changing server change. Server-only deployment, database, observability, and internal hardening can remain server-only. Sign-in/sign-out, magic links, OTP, factors, approvals, recovery, device/session state, tenant/account selection, delegated-token semantics, policy, notifications, errors, and navigation normally require coordinated client work or a documented exception.

Each issue and pull request records affected surfaces, omitted surfaces and rationale, parity gaps, follow-up work, and separate platform/release status.

## Deep links and OAuth/OIDC

Canonical app navigation:

```text
https://<verified-shared-auth-owned-host>/open/<route>?<bounded-query>
```

Non-OAuth fallback:

```text
sharedauth://<route>?<bounded-query>
```

The exact host must be verified. `sharedauth://` must not carry OAuth authorization responses unless a reviewed ADR replaces it with a collision-resistant owned-domain scheme.

OAuth/OIDC uses the external system browser, Authorization Code + PKCE, high-entropy state, nonce where applicable, and exact issuer/audience/provider/redirect validation. Prefer a verified claimed-HTTPS callback. Desktop fallback uses one ephemeral listener bound only to `127.0.0.1` or `[::1]`, then closes immediately. Embedded login WebViews are prohibited.

Flutter and Floem must consume the same versioned route types, redirect fixtures, OAuth/OIDC vectors, and replay/expiry tests. They support cold start, already-running delivery, authentication resume, and browser fallback.

Passwords, bearer/refresh tokens, redeemed authorization codes, client secrets, recovery secrets, TOTP seeds, private keys, session cookies, and sensitive identity data are prohibited in URLs.

## Review checklist

- [ ] Flutter Android/iOS impact evaluated.
- [ ] Flutter Web/mobile-web impact evaluated.
- [ ] Flutter desktop impact evaluated.
- [ ] Floem Rust desktop impact evaluated.
- [ ] Shared policy, client, OAuth/OIDC, route, and fixture impact evaluated.
- [ ] Browser authorization and deep-link compatibility tested where relevant.
- [ ] Omitted surfaces have an explicit rationale and follow-up when needed.

## Routing

- GitHub Project: [`shared-auth-project` — Project 1](https://github.com/orgs/shared-auth/projects/1)
- Linear project: [`github.com/shared-auth`](https://linear.app/denman/project/githubcomshared-auth-acbca07bb390)
- Central policy: [`cross-surface-delivery.md`](https://github.com/ORESoftware/project-registry/blob/main/docs/cross-surface-delivery.md)
- Desktop registry: [`desktop-applications.json`](https://github.com/ORESoftware/project-registry/blob/main/registry/desktop-applications.json)
