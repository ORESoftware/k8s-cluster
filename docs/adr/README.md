# Architecture decision records

Numbered, immutable-once-accepted decision records for this repository.
`docs/architecture/portfolio-boundaries.md` requires an ADR for boundary exceptions; this
directory is the canonical home for those records.

Conventions:

- Files are `NNNN-kebab-case-title.md`, numbered in creation order.
- Every ADR carries a Status line: `Draft`, `Proposed`, `Accepted`, `Rejected`, or
  `Superseded by ADR NNNN`.
- An accepted ADR names an owner, rationale, security review, and migration or expiry date.
- A `Draft` ADR with OPEN decisions records the decision framework only; OPEN questions may not be
  treated as decided, and pending evidence gates may not be marked pass/fail without linked
  evidence.
- No secret values, private identities, or real key material — synthetic placeholders only.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-secrets-backend-by-secret-class.md) | Secret backend by secret class for the k8s-cluster app-of-apps | Draft — all decisions OPEN pending DEN-2665 |
