# scintilla-run-monorepo

Private integration and GitOps authority for the
[`scintilla-run`](https://github.com/scintilla-run) function-runner system.
The repository shape follows the useful superproject patterns in Fiducia,
Daedalus, and Quaestor; the application and runtime code is Scintilla-specific.

Every deployable or integration component is an exact git submodule pin under
`apps/`, tracking its source repository's `main` branch:

- `gleam-lambda-runner` — Gleam/BEAM runtime, child processes, containers, and
  durable workflows.
- `scintilla-backend.rs` — Rust control-plane API.
- `scintilla-app-rs` — Rust MASH operator console.
- `scintilla-run-infra` — Cloudflare, Kubernetes, and Argo app-of-apps code.
- `scintilla-clients` — TypeScript, Rust/WASM, Dart, and Flutter clients.
- `scintilla-interfaces` — OpenAPI, AsyncAPI, JSON Schema, WIT, and generated
  language contracts.
- `scintilla-ui.dart` — Flutter UI.
- `scintilla-mcp-server.rs` — Rust stdio MCP server.
- `scintilla-sync` — additive definition reconciler.

`scintilla-run.github.io` is deliberately outside this monorepo because it is a
standalone public marketing site, not a Kubernetes workload.

## Clone and verify

```sh
git clone --recurse-submodules git@github.com:scintilla-run/scintilla-run-monorepo.git
cd scintilla-run-monorepo
npm test
```

## Deployment ownership

Individual repositories run tests only. The manually approved `deploy`
workflow here builds the exact pinned backend/app/UI/sync sources, publishes
immutable GHCR images, renders `scintilla-run-infra`, and commits only
`gitops/ec2` desired state. It has no kubeconfig and never calls `kubectl apply`;
Argo CD reconciles the committed app-of-apps release.

The monorepo is itself pinned by `ORESoftware/k8s-cluster` at
`remote/deployments/scintilla-run-monorepo` on that repository's `dev` branch.
