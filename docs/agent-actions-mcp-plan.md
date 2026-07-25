# agent-actions-mcp — rollout plan / next steps

Forward plan for `agent-actions-mcp-rs` (currently a local repo at
`/Users/maca5/codes/agent-actions-mcp-rs`, git-init'd, not yet a GitHub repo or a
`k8s-cluster` submodule). It is the **write-capable** MCP server for agent
automations — the counterpart to the read-only `remote/deployments/cluster-mcp-rs`.

Goal: one safety spine under four automations —
1. **Job search + applications** (fill forms, upload résumé),
2. **Gmail triage + response** for `hello@fiducia.cloud` + `alexander.d.mills@gmail.com` (avoid scams, never submit SSN),
3. **Lead discovery** (emails/phones) for `hello@benefactor.cc` / `github.com/benefactor-cc`,
4. **YouTube publishing** for `anticaptrad@gmail.com`.

Driven by ChatGPT (or Claude/Codex) over MCP, directly or through
`remote/deployments/ai-agent-bridge`.

## Current state (done)

The safety core is implemented and tested (28 tests, clippy clean, mock adapters):

- `src/firewall.rs` — hard-denies SSN / payment-card (Luhn) / bank-routing (ABA) / credentials / seed phrases in every outbound payload. Denied → terminal `blocked`, never approvable. Enforces "do not put in SSN" in code.
- `src/approval.rs` + `src/dispatch.rs` — propose → **pending** → human `approve` → `dispatch`. No path to a side effect skips approval.
- `src/safety.rs` — advisory phishing/scam annotator; never auto-decides.
- `src/mcp.rs` — MCP JSON-RPC surface, propose/approve/operator tool split for all four goals.
- `src/adapters.rs` — traits over the real services, with in-process mocks.
- `src/main.rs` — axum `/healthz /readyz /metrics /mcp`; loopback-only without `API_AUTH_BEARER`.

## Decisions needed from the operator (blockers)

| # | Decision / action | Blocks |
|---|---|---|
| D1 | Which GitHub org owns the repo (ORESoftware? fiducia.cloud, matching `ai-agent-bridge`?) | Phase 0 |
| D2 | Authorize the claude.ai Gmail/Drive connectors (or provision Google OAuth) | Phase 2, parts of 3 |
| D3 | Google OAuth for the `anticaptrad` YouTube channel | Phase 4 |
| D4 | NATS credentials + subject for `browser-job-runner-rs` from this service | Phase 1 |
| D5 | `web-scraper-service` URL + confirm lead queries stay within its `AGENTS.md` guardrails | Phase 3 |

## Phase 0 — land the service in the platform

- Push the repo to the chosen org (D1); register as a `k8s-cluster` submodule under `remote/deployments/` and add to `SUBMODULES.md`.
- Dockerfile + k8s manifests mirroring `cluster-mcp-rs` (Deployment, Service, NetworkPolicy, ExternalSecret for `API_AUTH_BEARER`); non-root, read-only rootfs, drop-ALL caps, no SA token.
- Real Prometheus metrics (proposed/approved/rejected/blocked/dispatched counters, firewall-deny by category) and register as a scrape target per the Observability Contract.
- Gateway exposure decision: keep the action surface **operator-authenticated only** (never the read-only IDE token used by `cluster-mcp`).
- **Done when:** deploys via ArgoCD, health/ready green, metrics scraped, reachable by an MCP client with a bearer.

## Phase 1 — job applications (recommended first; most self-contained)

- Implement `BrowserRunner` against `browser-job-runner-rs`: NATS request/reply to the browser-jobs pool subject; map `submit_form` / `upload_file` / `open_link` to job scenarios (D4).
- Résumé/profile store: a `resume_ref` → document mapping (start with a file/secret ref; the firewall already guards the answers).
- Job-board search adapter (start with one board) feeding `propose_apply_job`.
- **Done when:** a real posting can be applied to end-to-end with a human approving the submission; firewall provably blocks any SSN/card in answers.

## Phase 2 — Gmail triage + response

- Implement `Mailbox` against the Gmail API (preferred over browser-driving; robust, auditable) (D2).
- Ship **read-only first**: `gmail_scan` + `assess_email` over both inboxes, producing triage summaries + risk annotations. Safe to run autonomously.
- Then enable gated `propose_reply_email` / `propose_click_link` — never autonomous (inbox is attacker-controlled input; this is the prompt-injection surface).
- **Done when:** inbox triage runs unattended; every reply/click requires approval and shows firewall + scam findings.

## Phase 3 — lead discovery + outreach

- Implement `LeadSource` against `web-scraper-service`, strictly within its `AGENTS.md` guardrails (public/authorized sources, robots/ToS, minimize PII, prefer official APIs).
- Feed discovered leads into `propose_send_outreach` (gated). Add per-recipient rate limits + a suppression/opt-out list (CAN-SPAM/GDPR posture).
- **Done when:** leads for `benefactor.cc` are discovered within guardrails and outreach is queued for approval, never auto-sent.

## Phase 4 — YouTube publishing

- Implement `VideoPublisher` against the YouTube Data API (D3); `propose_publish_video` defaults to `private`.
- Video render/upload pipeline (file → `file_ref`), likely reusing existing media infra.
- **Done when:** a video can be published to `anticaptrad` only after approval.

## Cross-cutting (parallel to phases)

- **Persistence:** swap the in-memory `Queue` (`src/approval.rs`) for a durable store (Postgres in its own namespace per the pg-defs convention) so pending/audit survive restarts.
- **Approval UX:** a minimal operator surface to list/approve/reject (web page, or push notifications into `ai-agent-bridge`) — approval is the human bottleneck, make it fast.
- **Audit log:** append-only record of every proposal, decision, and dispatch (who/what/when + firewall verdict).
- **ChatGPT wiring:** expose `/mcp` as a ChatGPT connector; document the bridge path for Claude/Codex.
- **Defense-in-depth:** re-run the firewall immediately before `dispatch` (belt-and-suspenders); consider per-tool rate limits on `propose_*`.

## Guardrail posture (do not regress)

- Firewall categories are enforced in code, not prompts; SSN/card/routing/credential/seed = hard deny.
- Nothing reaches an adapter without human approval.
- The scam annotator informs; it never decides.
- Lead scraping stays inside `web-scraper-service` guardrails; surface conflicts rather than working around them.
