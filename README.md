# drone-mngr monorepo

This repository is the release inventory for the drone-mngr platform. The Rust applications and infrastructure remain independently versioned repositories and are pinned here as Git submodules.

| Path | Responsibility |
| --- | --- |
| `apps/drone-mngr-ctrl-server.rs` | Vendor-neutral drone, flight-policy, job, command, telemetry, and authorization API |
| `apps/drone-mngr-mcp-server.rs` | Authenticated MCP gateway for fleet, job, policy, and guarded flight-command tools |
| `apps/drone-mngr-web-server.rs` | MASH operator console: Maud, Axum, Supabase, SeaORM/Postgres, and HTMX |
| `apps/drone-mngr-infra` | Argo CD app-of-apps, Kubernetes integration, Cloudflare Worker, and OpenTofu |
| `tools/flags-2-env` | Pinned native CLI-to-environment parser used by every repository |

## Clone and develop

```sh
git clone --recurse-submodules https://github.com/drone-mngr/drone-mngr-monorepo.git
cd drone-mngr-monorepo
nix develop
scripts/check
```

If an existing clone is missing content, run `git submodule update --init --recursive`.

Every repository has a dot-prefixed `.nix/` directory, a root `.cli-flags.toml`, a `scripts/with-flags` launcher, and OpenTelemetry configuration. Runtime secrets stay outside Git.

## Release model

Application manifests live with the application they deploy. Argo CD points directly at each application repository, while this monorepo records a tested combination of their revisions. Commit an application change in its own repository first, then advance the corresponding gitlink here.

The bootstrap manifest is intentionally not applied automatically. See [docs/deployment.md](docs/deployment.md) for the guarded cluster handoff and [docs/architecture.md](docs/architecture.md) for system boundaries.
