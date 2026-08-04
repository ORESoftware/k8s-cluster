# zed-monorepo

Umbrella repo for [zed-pkg](https://zpkg.tech). Every zed-pkg repo is vendored
here as a **git submodule** under `apps/`, as siblings — which is exactly the
layout the Rust services need, since they path-depend on `../zed-interfaces`.

```
apps/
  zed-interfaces/       contract crate (path dep of the Rust services)
  zed-cli/              the `zed` CLI
  zed-api-server.rs/    registry REST API
  zed-web-server.rs/    registry web UI (MASH)
  zed-clients/          ten SDKs: Rust/WASM/TypeScript/Python/Go/Dart/Gleam/Erlang/Java/Swift
  zed-sync/             offline-first sync engine
  zed-infra/            terraform + k8s app-of-apps
  zed-docs/             architecture docs
  zed-e2e/              cross-stack e2e suites (playwright/puppeteer/selenium)
  zed-pkg.github.io/    marketing site (Astro)
```

This repo is itself included as a submodule of the cluster app-of-apps root
(`~/codes/ores/k8s-cluster`); see
[`apps/zed-infra/docs/wiring-k8s-cluster.md`](apps/zed-infra/docs/wiring-k8s-cluster.md).

## Clone

```sh
git clone --recurse-submodules https://github.com/zed-pkg/zed-monorepo.git
# or, after a plain clone:
git submodule update --init --recursive
```

## Common tasks

```sh
make init       # init/update all submodules
make pull       # update every submodule to its remote main
make test       # run each repo's test suite
make build      # cargo build the Rust services + build the TS packages
make images     # build the api/web container images (parent-context)
make status     # short git status across all submodules
```

## Why siblings under apps/

`zed-api-server.rs` and `zed-web-server.rs` declare
`zed-interfaces = { path = "../zed-interfaces" }`. With every repo a sibling
under `apps/`, that path resolves both for local `cargo` builds and for the
Docker builds, whose context is `apps/`:

```sh
docker build -f apps/zed-api-server.rs/Dockerfile -t ghcr.io/zed-pkg/zed-api-server:dev apps
```

## Integration CI, and why contract changes go first

Each repo's own CI checks out `zed-pkg/zed-interfaces` at its
**default-branch tip**, which floats. That answers "does this commit work
against the newest contract?" but has two consequences worth knowing:

1. **Push order matters.** When a change spans the contract and a consumer,
   push `zed-interfaces` **first**. Push the consumer first and its CI
   compiles against a contract that lacks the new item, so the run fails for
   a reason unrelated to the commit. (Re-running after the contract lands
   turns it green — the code was never wrong.)
2. **Nothing else verifies a *combination*.** Only this repo pins exact SHAs.

So [`.github/workflows/integration.yml`](.github/workflows/integration.yml)
builds and tests the pinned set here, where `apps/` already provides the
sibling layout `../zed-interfaces` needs — no floating checkout anywhere. It
also runs the cross-stack Playwright suite (Postgres + both servers + the
CLI). It runs on push/PR, nightly, and on demand, since the change that
invalidates a pin usually lands in a *different* repo.

## Portfolio inventory ratchet

`.gitmodules` is the executable source of truth for the exact sibling set.
[`scripts/check-portfolio-inventory.py`](scripts/check-portfolio-inventory.py)
compares those gitlinks with the human-readable `apps/` inventory above and
fails on missing, duplicate, renamed, or undocumented repositories. The check
is deliberately narrower than the full governed-fleet catalog in DEN-627: it
protects this exact pinned integration set without creating a second portfolio
registry.

## License

MIT
