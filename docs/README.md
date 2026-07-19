# docs

Cross-cutting documentation for the `fiducia-monorepo` superproject — the
concerns that span the whole fleet rather than any single app repo.

- `repo-boundaries.md` — intended per-repo visibility, the live audited
  visibility snapshot, and the rules for keeping public contracts separate from
  private history.
- `deploy.md` — the two-environment model: app repos own TEST, the monorepo owns
  a single manual PROD deploy.
- `k8s-cluster-gitops-todos-2026-07.md` — the safe ownership-transfer checklist
  for moving ORES web-plane desired state into this monorepo.
- `SECURITY-AUDIT.md` — static security review across the pinned app repos.
- `future-work.md` — storage decisions and highest-value product gaps.
- `use-cases-exploration.md` — speculative fit analysis of proposed product
  directions against Fiducia's coordination primitives.
- `messaging-consensus-observability.md` — enforceable boundary between Raft
  authority and NATS delivery, plus the fleet telemetry and failure contract.

Some of these files are asserted by the contract tests in `tests/` (e.g.
`repo-boundaries.md` must classify every submodule).
