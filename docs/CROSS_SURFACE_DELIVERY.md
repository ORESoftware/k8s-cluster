# Cross-surface delivery

Verified **2026-08-06**.

## Surfaces

- Rust analytics/web/operator server: `claritas-viz/data-viz-server.rs`
- Flutter Android/iOS, Flutter Web/mobile web, and Flutter desktop: `claritas-viz/claritas-flutter` — planned
- Rust desktop workbench: `claritas-viz/claritas-desktop.rs` — planned Dioxus Desktop application
- Shared contracts: Claritas interfaces, generated clients, dataset/query/semantic-model/dashboard/chart/alert/report/export schemas, route types, renderer fixtures, and conformance tests

## Judgment-based propagation

Evaluate Flutter mobile, Flutter Web, Flutter desktop, Rust desktop, and shared contracts for every user-visible or contract-changing web/API change. Query-engine internals, server-only cache/storage, parser implementation, observability, and artifact-worker plumbing may remain server-only. Native local datasets, multi-window analysis, local files, keyboard workflows, GPU/native overlays, and offline work may be desktop-specific. Dataset and field semantics, query results, semantic models, dashboard/chart behavior, alerts, notifications, publishing approvals, reports, exports, permissions, errors, and navigation normally propagate or require an explicit rationale and parity issue.

Mobile does not need every dense desktop authoring surface. Good judgment may keep complex workbook or infrastructure-diagram editing on web/desktop while mobile receives view, filter, notification, approval, and deep-link workflows. Each issue and pull request records affected surfaces, omitted surfaces and rationale, accepted parity gaps, follow-up work, and separate platform/release status.

## Deep links

Canonical:

```text
https://<verified-claritas-owned-host>/open/<route>?<bounded-query>
```

Fallback:

```text
claritas://<route>?<bounded-query>
```

The HTTPS host must be verified before publication. All surfaces share versioned route types and golden fixtures and support cold start, already-running delivery, authentication resume, replay/expiry rejection, browser fallback, and explicit confirmation before connection changes, publication/review, alert actions, imports, exports, or destructive operations.

Never put raw datasets, query text containing secrets, connection credentials, secretRef values, private logs, alert destinations, report contents, rendered private artifacts, bearer tokens, or database identifiers in URLs. Use bounded identifiers or short-lived, single-use, audience-bound codes and validate route version, dataset/model/dashboard/chart/report/alert/connection IDs, action, authorization, role, limits, and user intent.

## Review checklist

- [ ] Flutter Android/iOS impact evaluated.
- [ ] Flutter Web/mobile-web impact evaluated.
- [ ] Flutter desktop impact evaluated.
- [ ] Dioxus Rust desktop impact evaluated.
- [ ] Shared query/model/dashboard/client/route/fixture impact evaluated.
- [ ] Deep-link and renderer/export compatibility tested where relevant.
- [ ] Dense authoring features omitted from mobile have a documented UX rationale.
- [ ] Omitted surfaces have a follow-up when needed.

## Routing

- GitHub Project: [`claritas-viz-project` — Project 1](https://github.com/orgs/claritas-viz/projects/1)
- Linear project: [`github.com/claritas-viz`](https://linear.app/denman/project/githubcomclaritas-viz-09fcc5d7dd9e)
- Central policy: [`cross-surface-delivery.md`](https://github.com/ORESoftware/project-registry/blob/main/docs/cross-surface-delivery.md)
- Desktop registry: [`desktop-applications.json`](https://github.com/ORESoftware/project-registry/blob/main/registry/desktop-applications.json)
