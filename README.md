# shared-auth monorepo

This repository is the pinned integration inventory for the shared-auth system. Runtime components remain independently versioned and released; reviewed source revisions are represented beneath `apps/` as Git submodules.

The workspace deliberately uses two non-overlapping dependency mechanisms:

- Zed installs `shared-auth-interfaces`, `shared-auth-lib`, and `shared-auth-clients` below `.vendor/.zed`.
- Git pins source repositories below `apps/` for coordinated integration and end-to-end verification.

Infrastructure and CLI repositories are not monorepo imports. Infrastructure remains independently deployed and owned by `shared-auth-infra`; a future `shared-auth-cli` must consume the three Zed packages from its own repository rather than becoming a monorepo dependency.

## Components

| Path | Responsibility |
| --- | --- |
| `apps/shared-auth-server.rs` | Rust Postgres-primary auth authority and Supabase exchange |
| `apps/shared-auth-lib` | Provider-neutral verification and multi-authority helpers |
| `apps/shared-auth-interfaces` | Schemas, fixtures, and generated cross-language contracts |
| `apps/shared-auth-clients` | Hardened polyglot application clients |
| `apps/shared-auth-mcp-server.rs` | Read-only Rust MCP server |
| `apps/shared-auth-nats-bridge.rs` | Rust bridge for auth lifecycle events over NATS |
| `apps/shared-auth-e2e` | Independent Playwright, Puppeteer, and Selenium suites |
| `apps/shared-auth-sync` | Offline-first queues and Postgres/Supabase reconciliation |

The dormant `shared-auth.github.io` repository is intentionally excluded: the organization does not need a GitHub Pages marketing site.

## Clone and update

```sh
git clone --recurse-submodules git@github.com:shared-auth/shared-auth-monorepo.git
cd shared-auth-monorepo
nix develop ./.nix
git submodule update --init --recursive
```

The checked-in gitlinks are pinned deployment and integration inputs. Optional `branch = main` metadata only guides intentional updates; normal clones remain fixed to exact reviewed commits.

Run `python3 scripts/verify-zed-submodules.py` before committing topology changes. CI rejects missing or undeclared gitlinks, Zed dependency drift, path overlap, and any infrastructure or CLI import.

## Operations

`.cli-flags.toml` is the flags-2-env contract for orchestration. OTLP variables are passed through to component commands; the monorepo itself does not install a second global OpenTelemetry provider.

For local development, enter `nix develop ./.nix` here or enter the more specific `.nix` shell inside a component. Do not place source directories or vendored copies outside the declared ownership roots.
