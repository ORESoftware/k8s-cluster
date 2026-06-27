# Repository Boundaries

`fiducia-monorepo` is the private integration and GitOps superproject. It pins
every Fiducia app repository to exact commits under `apps/`, but it is not the
source of truth for component ownership. Each app repo keeps its own visibility,
history, CI, issue surface, and release permissions.

This separation lets Fiducia make selected components open source without
exposing private control-plane, deployment, customer, or security-sensitive
history.

## Visibility Defaults

Public or public-ready repositories:

- `fiducia-interfaces`: protocol, schema, generated language contracts.
- `fiducia-clients`: public SDKs and protocol documentation.
- `fiducia-cli.rs`: developer CLI and closest-region tooling.
- `fiducia-routing.rs`: region enum and deterministic routing helpers.
- `fiducia-ui.web`: public marketing/product web surface.

Private repositories by default:

- `fiducia-monorepo`: all-up GitOps pins, including private submodules.
- `fiducia-infra`: cluster topology, generated deployment state, and ops docs.
- `fiducia-node.rs`: core coordination engine and shard internals.
- `fiducia-brain.rs`: control plane and placement logic.
- `fiducia-load-balance.rs`: leader-routing and fleet topology behavior.
- `fiducia-auth.rs`: auth integration, key handling, and trust policy.
- `fiducia-admin.rs`: internal admin and operator APIs.
- `fiducia-backend.rs`: customer portal backend integration.
- `fiducia-customer-ui.web`: authenticated customer portal.
- `fiducia-node-sidecar.rs`: node-local bridge and heartbeat logic.
- `fiducia-edge`: edge routing and deployment-adjacent policy.
- `fiducia-telemetry.rs`: internal tracing conventions and service metadata.

## Rules

- Keep the all-up superproject private unless every submodule URL and pin is
  safe for public consumption.
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
