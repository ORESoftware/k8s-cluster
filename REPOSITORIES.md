# Shared Auth repository and release map

This inventory classifies every canonical repository in the `shared-auth` organization. Repositories remain independently releasable; the monorepo pins compatible application-source commits under `apps/` and installs shared package dependencies under `.vendor/.zed`.

| Repository | Class | Artifact or deployment | Relationship |
|---|---|---|---|
| `shared-auth-interfaces` | canonical contracts | generated Rust, TypeScript, Go, JavaScript, Dart, and Gleam types | source of truth for wire outcomes and identity shapes; Zed dependency and source submodule |
| `shared-auth-clients` | client SDKs | one package per language through the Zed package manifest | consumes interfaces and lib; Zed dependency and source submodule |
| `shared-auth-lib` | consumer guards | source libraries; currently private and unpublished | consumes generated interfaces; Zed dependency and source submodule |
| `shared-auth-server.rs` | Rust authority service | `ghcr.io/shared-auth/shared-auth-server` pinned by digest in `deploy/k8s` | Postgres authority; Supabase secondary authority; Redis acceleration; source submodule |
| `shared-auth-nats-bridge.rs` | Rust event bridge | `ghcr.io/shared-auth/shared-auth-nats-bridge`; namespace-scoped Kubernetes manifests | translates auth lifecycle delivery; source submodule |
| `shared-auth-sync` | offline/convergence engine | Rust core plus TypeScript and Dart/Flutter clients and Postgres migration | synchronizes optimistic local state and provider mirrors; source submodule |
| `shared-auth-mcp-server.rs` | Rust operator tooling | private MCP binary/container candidate | diagnostics over server and bridge APIs; source submodule |
| `shared-auth-infra` | edge and cloud infrastructure | Cloudflare Worker and Terraform plans | independently deployed and intentionally excluded from monorepo imports |
| `shared-auth-e2e` | black-box verification | independent Playwright, Puppeteer, and Selenium suites | validates deployed behavior; source submodule |
| `shared-auth-monorepo` | integration inventory | Zed package plus reviewed Git submodule pins | depends only on interfaces, lib, and clients; never imports infra or CLI |
| `shared-auth.github.io` | dormant reserved website | no GitHub Pages deployment | organization-name reservation and policy surface |

## Release order

1. Merge and tag interface changes, then regenerate and validate every language.
2. Update lib, clients, and sync consumers against the exact contract revision.
3. Build service and bridge images once, record immutable digests, and let repository-owned GitOps manifests promote them.
4. Advance monorepo package versions and source gitlinks only after component CI passes.
5. Run all three independent browser suites against the promoted environment.

Provider API keys, signing material, database URLs, cache URLs, and internal service credentials are never release artifacts. Fiducia/runtime secret delivery injects them as environment variables. Browser artifacts may contain publishable provider keys but never service-role or server-side secret keys.

## Current exceptions and follow-up

- The website repository is deliberately dormant; policy CI is required, but a Pages or marketing deployment is not.
- The MCP server has CI but no declared production deployment. Add an Argo application only when an operator-facing runtime is approved.
- `shared-auth-cli` does not yet exist and must be created as a separate Zed package consuming interfaces, lib, and clients.
- The legacy `ORESoftware/shared-auth-server.rs` history must remain intact until unique commits and provenance are reconciled with the canonical server.
