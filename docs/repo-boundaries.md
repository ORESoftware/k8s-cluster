# Repository Boundaries

`fiducia-monorepo` is the integration and GitOps superproject and is intended to
be private. It pins every Fiducia app repository to exact commits under `apps/`,
but it is not the source of truth for component ownership. Each app repo keeps
its own visibility, history, CI, issue surface, and release permissions.

This separation lets Fiducia make selected components open source without
exposing private control-plane, deployment, customer, or security-sensitive
history.

## Intended visibility defaults

Public or public-ready repositories:

- `fiducia-interfaces`: protocol, schema, generated language contracts.
- `fiducia-clients`: public SDKs and protocol documentation.
- `fiducia-sync`: local-first sync SDK (`@fiducia/sync`) — a zero-IO Rust
  reconcile core (also compiled to WASM) plus a thin TS shim. Consumes the
  `fiducia-interfaces` row/change-event contracts; ships no private history.
- `fiducia-test-config`: shared `node --test` browser-test harness and
  tsconfig/eslint presets (`@fiducia/test-config`). Dev-tooling only, no
  product or control-plane code — public-ready like `fiducia-interfaces`.
- `fiducia-cli.rs`: developer CLI and closest-region tooling.
- `fiducia-mcp-server.rs`: read-only MCP server (`fiducia-mcp`, stdio) giving
  MCP-capable agents diagnostics over brain/node/agent-control-plane plus an
  embedded repo map; no mutating tools, public-ready.
- `fiducia-memory.rs`: contestable claim ledger, governed durable memory, and
  explainable hybrid recall primitives and service APIs.
- `fiducia-messaging.rs`: reusable NATS subject, envelope, fencing, deduplication,
  and optional Postgres/JetStream relay primitives.
- `fiducia-payments.rs`: provider-agnostic Stripe and PayPal webhook signature
  verification and event parsing; pure library code with no server or database.
- `fiducia-ai-agent-manager.rs`: AI-agent lifecycle/dispatch manager; server-auth
  gated, with per-provider child-env isolation.
- `fiducia-lambda-service.rs`: sandboxed function/runner dispatch service with
  request-body limits and server-auth-gated mutating routes.
- `fiducia-routing.rs`: region enum and deterministic routing helpers.
- `fiducia-marketing.web`: public marketing/product web surface.

Private repositories by default:

- `fiducia-monorepo`: all-up GitOps pins, including private submodules.
- `fiducia-infra`: cluster topology, generated deployment state, and ops docs.
- `fiducia-node.rs`: core coordination engine and shard internals.
- `fiducia-brain.rs`: control plane and placement logic.
- `fiducia-load-balance.rs`: leader-routing and fleet topology behavior.
- `fiducia-auth.rs`: auth integration, key handling, and trust policy.
- `fiducia-admin.rs`: internal admin and operator APIs.
- `fiducia-ai-agent-bridge.rs`: customer-operated, topic-routed AI-agent
  conversation bus, transport hardening, and optional message persistence.
- `fiducia-ai-agent-control-plane`: single-tenant, customer-operated agent
  orchestration, source context, model workflows, memory, and audit state.
- `fiducia-customer.rs`: customer portal backend integration.
- `fiducia-customer-ui.web`: deprecated legacy customer SPA (archived; superseded by `fiducia-customer.rs`, no longer a monorepo submodule).
- `fiducia-node-sidecar.rs`: node-local bridge and heartbeat logic.
- `fiducia-operations-control-plane`: single-tenant deployment, migration,
  scheduling, runner, rollout, and infrastructure audit state.
- `fiducia-edge`: edge routing and deployment-adjacent policy.
- `fiducia-e2e`: cross-cluster conformance and chaos test orchestration.
- `fiducia-telemetry.rs`: internal tracing conventions and service metadata.

## Live visibility snapshot

The live GitHub audit on 2026-07-13 found four private application repositories:
`fiducia-ai-agent-bridge.rs`, `fiducia-ai-agent-control-plane`, `fiducia-e2e`,
and `fiducia-operations-control-plane`. Every other repository in this workspace,
including `fiducia-monorepo`, was public. That is a policy mismatch for twelve
repositories listed as private-by-default above. Visibility is an owner-level
release decision because changing it affects forks, collaborators, disclosure,
and automation; this code audit therefore reports the mismatch without changing
GitHub settings.

Trusted `main` fleet-audit runs and production checkout use a read-only
fine-grained `FIDUCIA_SUBMODULE_TOKEN` so private submodules are never silently
omitted from a deployable-state audit. Public PR contract CI intentionally
initializes only `fiducia-interfaces` and `fiducia-sync`; it still verifies all
27 declarations as exact gitlinks without requesting private repository data.
The token belongs in GitHub Environments/secrets, never in this repository.

## Rules

- Restore the all-up superproject to private unless an owner explicitly accepts
  the audited public posture and confirms every submodule URL and pin is safe
  for public consumption.
- Use a separate public-only superproject if contributors need a single checkout
  across public components.
- Do not commit real `.env*` files, private keys, tokens, certificates, or
  generated secret bundles. Use `.env.example` for placeholders and keep real
  values in ignored local files or secret managers.
- Treat submodule pins as deployable state. A component change is not deployable
  through GitOps until the component repo is pushed and the superproject pin is
  updated.
- Open-source candidates should keep public contracts in `fiducia-interfaces`
  and SDK repos. Private repos may consume those contracts without leaking
  implementation details back into public history.
