# shared-auth monorepo

This repository is the pinned inventory for the shared-auth system. Every
component lives beneath `apps/` as an independent Git submodule; component
history, releases, dependencies, OpenTelemetry policy, and CI remain owned by
the component repository.

## Components

| Path | Responsibility |
| --- | --- |
| `apps/shared-auth-server.rs` | Rust Postgres-primary auth authority and Supabase exchange |
| `apps/shared-auth-lib` | Provider-neutral Rust verification and multi-authority helpers |
| `apps/shared-auth-interfaces` | Schemas, fixtures, and generated cross-language contracts |
| `apps/shared-auth-clients` | Hardened application clients |
| `apps/shared-auth-infra` | Cloudflare Worker and AWS/Kubernetes integration guidance |
| `apps/shared-auth-mcp-server.rs` | Read-only Rust MCP server |
| `apps/shared-auth-nats-bridge.rs` | Rust bridge for auth lifecycle events over NATS |
| `apps/shared-auth-e2e` | Independent Playwright, Puppeteer, and Selenium suites |
| `apps/shared-auth-sync` | Offline-first client queues and Postgres/Supabase reconciliation |

The dormant `shared-auth.github.io` repository is intentionally excluded: the
organization does not need a GitHub Pages marketing site.

## Clone and update

```sh
git clone --recurse-submodules git@github.com:shared-auth/shared-auth-monorepo.git
cd shared-auth-monorepo
nix develop ./.nix
git submodule update --init --recursive
```

The checked-in gitlinks are the deployment inputs. The optional `branch = main`
metadata only guides an intentional remote update; normal clones remain pinned
to the exact reviewed commits. All component pointers track reviewed `main`
revisions. The server pointer includes the release workflow's immutable GHCR
digest pin rather than an unreviewed floating image tag.

## Operations

`.cli-flags.toml` is the flags-2-env contract for orchestration. OTLP variables
are passed through to component commands; the monorepo itself does not install
a second global OpenTelemetry provider.

For local development, enter `nix develop ./.nix` here or enter the more
specific `.nix` shell inside a component. Do not place source directories or
vendored copies outside `apps/`.
