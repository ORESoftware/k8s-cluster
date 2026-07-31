# sonus-auris-monorepo

Git superproject for the [Sonus Auris](https://github.com/sonus-auris) repositories —
a dashcam for audio. Each app/service repo is tracked as a git submodule under
[`apps/`](apps/). The superproject pins each submodule to an exact commit, while
`.gitmodules` sets `branch = main` for every submodule so updates intentionally
follow each repo's `main` branch.

This is the private all-up integration view. Individual app repos keep their own
visibility, so public repos (UI, marketing site) coexist with private ones
(backend, interfaces, desktop, infra).

## Apps

| App | Path | Source |
|-----|------|--------|
| Backend (Rust) | [`apps/sonus-auris-backend.rs`](apps/sonus-auris-backend.rs) | `sonus-auris/sonus-auris-backend.rs` |
| Account web server (Maud/Axum/SeaORM/HTMX) | [`apps/sonus-auris-web-server.rs`](apps/sonus-auris-web-server.rs) | `sonus-auris/sonus-auris-web-server.rs` |
| UI (Dart/Flutter) | [`apps/sonus-auris-ui.dart`](apps/sonus-auris-ui.dart) | `sonus-auris/sonus-auris-ui.dart` |
| Shared interfaces | [`apps/sonus-auris-interfaces`](apps/sonus-auris-interfaces) | `sonus-auris/sonus-auris-interfaces` |
| Marketing site | [`apps/sonus-auris-site.web`](apps/sonus-auris-site.web) | `sonus-auris/sonus-auris-site.web` |
| Desktop app (Rust) | [`apps/desktop.app.rs`](apps/desktop.app.rs) | `sonus-auris/desktop.app.rs` |
| Console app (Dart/Flutter, desktop + web) | [`apps/sonus-auris-web-desktop.dart`](apps/sonus-auris-web-desktop.dart) | `sonus-auris/sonus-auris-web-desktop.dart` |
| API server (Rust, JSON + billing webhooks) | [`apps/sonus-auris-api-server.rs`](apps/sonus-auris-api-server.rs) | `sonus-auris/sonus-auris-api-server.rs` |
| Cloudflare proxy for `sonusauris.app` | [`apps/sonusauris-app-proxy`](apps/sonusauris-app-proxy) | `ORESoftware/sonusauris-app-proxy` |
| Infra (k8s) | [`apps/sonus-auris.infra`](apps/sonus-auris.infra) | `sonus-auris/sonus-auris.infra` |

## Docs

- [`docs/contractor-work-intelligence/`](docs/contractor-work-intelligence/) —
  product and engineering handbook for the separately branded contractor sister
  app: product scope, architecture, domain model, field UX, privacy/trust,
  deterministic reports and billing, quality gates, roadmap, glossary, and ADRs.
- [`docs/DEPLOY.md`](docs/DEPLOY.md) — how each app deploys.
- [`docs/FOLLOWUPS.md`](docs/FOLLOWUPS.md) — what still needs finishing (the
  paid-tier server enforcement, mobile passwordless UI, web MFA screen, …),
  prioritized with how-to.
- [`docs/HARDENING.md`](docs/HARDENING.md) — security / robustness / supply-chain
  shore-up list.
- [`docs/SUPABASE_SETUP.md`](docs/SUPABASE_SETUP.md) — the dashboard config that
  must be done by hand (MFA, SMS, email template).

## Shared CLI flags

[`tools/flags-2-env`](tools/flags-2-env) pins
[`ORESoftware/flags-2-env`](https://github.com/ORESoftware/flags-2-env) for the
whole workspace. Every app repository declares its own `.cli-flags.toml` and
ships the same strict wrapper:

```sh
scripts/with-flags help
scripts/with-flags audit
scripts/with-flags --web-port=8130 -- \
  cargo run --manifest-path apps/sonus-auris-web-server.rs/Cargo.toml
```

The wrapper compiles the pinned native source into a commit-keyed user cache,
rejects unknown or invalid options, and then exports the validated map to the
downstream command. CLI values override inherited environment values. Secrets
remain environment-only and are excluded from the declared flag surfaces.

## Clone

```sh
git clone --recurse-submodules git@github.com:sonus-auris/sonus-auris-monorepo.git
```

For an existing checkout:

```sh
git submodule update --init --recursive
```

## Dev environment (Nix)

A flake provides a dev shell with the Rust, Dart, and Node toolchains plus the
native build deps used across the apps:

```sh
nix develop
```

Or, with [direnv](https://direnv.net/), `direnv allow` once and the shell loads
automatically on `cd` (see [`.envrc`](.envrc)).

## Update submodule pins to latest `main`

```sh
git submodule update --remote --merge
git add apps
git commit -m "Pin Sonus Auris apps to main"
```