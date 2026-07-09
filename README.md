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
| UI (Dart/Flutter) | [`apps/sonus-auris-ui.dart`](apps/sonus-auris-ui.dart) | `sonus-auris/sonus-auris-ui.dart` |
| Shared interfaces | [`apps/sonus-auris-interfaces`](apps/sonus-auris-interfaces) | `sonus-auris/sonus-auris-interfaces` |
| Marketing site | [`apps/sonus-auris-site.web`](apps/sonus-auris-site.web) | `sonus-auris/sonus-auris-site.web` |
| Desktop app (Rust) | [`apps/desktop.app.rs`](apps/desktop.app.rs) | `sonus-auris/desktop.app.rs` |
| Infra (k8s) | [`apps/sonus-auris.infra`](apps/sonus-auris.infra) | `sonus-auris/sonus-auris.infra` |

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
