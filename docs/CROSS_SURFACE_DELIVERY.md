# Cross-surface delivery

Verified **2026-08-06**.

## Surfaces

- Rust web portal: `akrion-sim/akrion-web-server.rs`
- Flutter Android/iOS, Flutter Web, and Flutter desktop: `akrion-sim/akrion-flutter` — proposed/planned candidate
- Rust desktop workbench: `akrion-sim/akrion-desktop.rs` — proposed/planned candidate
- Shared contracts: Akrion interfaces, generated clients, scenario/model/run schemas, deterministic seeds, routes, result bundles, and conformance fixtures

Repository names are allocation targets until their remotes and native builds are verified.

## Judgment-based propagation

Evaluate mobile, Flutter Web, Flutter desktop, Rust desktop, and shared contracts for every user-visible or contract-changing web change. Web portal presentation and web-only administration may remain web-only. Local datasets, file handling, large batch runs, offline replay, and native rendering may be native-specific. Scenario/model semantics, run state, deterministic replay, result interpretation, permissions, errors, notifications, and navigation normally propagate or require an explicit rationale and parity issue.

## Deep links

```text
https://<verified-akrion-owned-host>/open/<route>?<bounded-query>
```

The host must be verified. A custom-scheme fallback requires a reviewed ADR and must not be guessed. All surfaces share versioned route types and fixtures and support cold start, already-running delivery, authentication resume, replay/expiry rejection, browser fallback, and explicit confirmation before imports, exports, execution, or destructive actions.

Never put private datasets, result payloads, credentials, tokens, absolute local paths, or sensitive simulation inputs in URLs. Use bounded identifiers or short-lived, single-use, audience-bound handoff codes and validate route version, scenario/model/run IDs, action, authorization, limits, and user intent.

## Review checklist

- [ ] Flutter Android/iOS impact evaluated.
- [ ] Flutter Web/mobile-web impact evaluated.
- [ ] Flutter desktop impact evaluated.
- [ ] Rust desktop workbench impact evaluated.
- [ ] Shared scenario/client/route/fixture impact evaluated.
- [ ] Deep-link compatibility tested where relevant.
- [ ] Omitted surfaces have a rationale and follow-up when needed.

## Routing

- GitHub Project: [`akrion-sim-project` — Project 1](https://github.com/orgs/akrion-sim/projects/1)
- Linear project: [`github.com/akrion-sim`](https://linear.app/denman/project/githubcomakrion-sim-c66c5e5e8f12)
- Central policy: [`cross-surface-delivery.md`](https://github.com/ORESoftware/project-registry/blob/main/docs/cross-surface-delivery.md)
- Desktop registry: [`desktop-applications.json`](https://github.com/ORESoftware/project-registry/blob/main/registry/desktop-applications.json)
