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
  zed-clients/          SDKs (rust/ts/python/go)
  zed-sync/             offline-first sync engine
  zed-infra/            terraform + k8s app-of-apps
  zed-docs/             architecture docs
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

## License

MIT
