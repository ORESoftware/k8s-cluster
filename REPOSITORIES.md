# Shared Auth repository and release map

This inventory classifies every canonical repository in the `shared-auth`
organization. Repositories remain independently releasable; the monorepo pins
compatible commits under `apps/` for integration and Kubernetes inventory.

| Repository | Class | Artifact or deployment | Relationship |
|---|---|---|---|
| `shared-auth-interfaces` | canonical contracts | generated Rust, TypeScript, Go, JavaScript, Dart, and Gleam types | source of truth for wire outcomes and identity shapes |
| `shared-auth-clients` | client SDKs | one package per language through the Zed package manifest | consumes interfaces and the server HTTP contract |
| `shared-auth-lib` | consumer guards | source libraries; currently private and unpublished | consumes generated interfaces; distinguishes invalid from degraded authorities |
| `shared-auth-server.rs` | Rust authority service | `ghcr.io/shared-auth/shared-auth-server` pinned by digest in `deploy/k8s` | Postgres authority; Supabase secondary authority; Redis acceleration; Argo CD tracks the repository directly |
| `shared-auth-nats-bridge.rs` | Rust event bridge | `ghcr.io/shared-auth/shared-auth-nats-bridge`; namespace-scoped Kubernetes manifests | translates authenticated HTTP publications and NATS lifecycle delivery; Argo CD tracks the repository directly |
| `shared-auth-sync` | offline/convergence engine | Rust core plus TypeScript and Dart/Flutter clients and Postgres migration | synchronizes optimistic local state, RDS Postgres, and provider mirrors using shared JSON Schema |
| `shared-auth-mcp-server.rs` | Rust operator tooling | private MCP binary/container candidate; no production Argo application yet | diagnostics over server and bridge APIs; never a browser credential authority |
| `shared-auth-infra` | edge and cloud infrastructure | Cloudflare Worker and Terraform plans | routes/races authorities at the edge and owns DNS/cloud resources, not Kubernetes application objects |
| `shared-auth-e2e` | black-box verification | independent Playwright, Puppeteer, and Selenium suites | validates deployed behavior; suites share no runtime helpers or fixtures |
| `shared-auth-monorepo` | integration inventory | Git submodule pins only; no independently published runtime | records a compatible multi-repository set and is itself pinned by `k8s-cluster` |
| `shared-auth.github.io` | dormant reserved website | no GitHub Pages deployment and no marketing release | retained as an organization-name reservation and policy/documentation surface |

## Release order

1. Merge and tag interface changes, then regenerate and validate every language.
2. Update guards, clients, and sync consumers against the exact contract
   revision.
3. Build service and bridge images once, record immutable digests, and let
   repository-owned GitOps manifests promote them.
4. Advance monorepo submodule pins only after component CI passes.
5. Run all three independent browser suites against the promoted environment.

Provider API keys, signing material, database URLs, cache URLs, and internal
service credentials are never release artifacts. Fiducia/runtime secret
delivery injects them as environment variables. Browser artifacts may contain
publishable provider keys but never service-role or server-side secret keys.

## Current exceptions and follow-up

- The website repository is deliberately dormant; policy CI is required, but a
  Pages or marketing deployment is not.
- The MCP server has CI but no declared production deployment. Add an Argo
  application only when an operator-facing runtime is approved.
- The legacy `ORESoftware/shared-auth-server.rs` history must remain intact
  until unique commits and provenance are reconciled with the canonical server.
- Coordinated package publication, rollback evidence, and legacy server-history
  reconciliation remain tracked by DEN-606.
