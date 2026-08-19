# ADR 0001: Secret backend by secret class for the k8s-cluster app-of-apps

- Status: **Draft — skeleton only. Every decision below is OPEN. No decision has been made.**
- Owner: OPEN — to be named at acceptance (`docs/architecture/portfolio-boundaries.md` requires an
  ADR to name an owner, rationale, security review, and migration or expiry date).
- Date opened: 2026-08-08
- Linear: [DEN-2666](https://linear.app/denman/issue/DEN-2666) (this ADR),
  [DEN-2665](https://linear.app/denman/issue/DEN-2665) (evidence source — rotation, failure,
  crossover, and soak matrix), [DEN-2661](https://linear.app/denman/issue/DEN-2661) (parent A/B),
  [DEN-2662](https://linear.app/denman/issue/DEN-2662) (Lane A canary),
  [DEN-2663](https://linear.app/denman/issue/DEN-2663) (Lane B canary).
- Related, not duplicated here: [DEN-2636](https://linear.app/denman/issue/DEN-2636) (SOPS
  `.env.enc` namespace), [DEN-1378](https://linear.app/denman/issue/DEN-1378) (fiducia-cloud
  encrypted single-file secrets), [DEN-630](https://linear.app/denman/issue/DEN-630) (app-of-apps
  contract), [DEN-1236](https://linear.app/denman/issue/DEN-1236) and the Fiducia hardening lane,
  [DEN-2897](https://linear.app/denman/issue/DEN-2897) / [DEN-2901](https://linear.app/denman/issue/DEN-2901)
  (owner-scoped secret roots and namespace gate).

## Purpose and evidence contract

This ADR will convert the DEN-2665 A/B evidence into an approved secret-classification policy and a
bounded rollout/rollback plan for the k8s-cluster app-of-apps: which secret classes (if any) may use
SOPS ciphertext in Git (Lane A), which default to Fiducia/ESO runtime delivery (Lane B), and which
must remain in the cloud secret manager.

**As of 2026-08-08, DEN-2665 has not started (Todo) and has produced no evidence. This document
therefore records the decision framework and open questions only.** The ADR may end in a hybrid
classification; it must not force a single mechanism if the evidence supports different mechanisms
for different secret classes. No secret values, private identities, or real key material may ever
appear in this document, its examples, Linear, or the evidence.

Per-lane disposition vocabulary (to be assigned per secret class only after evidence):

- approved for production;
- approved only for development/bootstrap;
- approved only after named blockers;
- rejected.

## Fixed constraints (not open — inherited from owning tickets)

These are inputs to the decisions below, owned elsewhere; this ADR must not relax or widen them:

1. **Dotenv namespace (DEN-2636, restated on DEN-2666, 2026-08-07):** if application dotenv
   ciphertext in Git is approved at all, the authoritative ORESoftware VCS namespace is exactly
   `env/enc/dev.env.enc` and `env/enc/prod.env.enc`, with all plaintext dotenv ignored and the root
   `.env` managed as a relative symlink. No third tracked `.env.enc` environment may be added to
   represent release/deployment/bootstrap planes.
2. **Artifact-class separation (DEN-2666 comment, 2026-08-07):** Kubernetes/GitOps encrypted Secret
   YAML (KSOPS) is a distinct artifact class from application dotenv. Any KSOPS approval must be a
   separate, narrowly scoped path/policy and must not change or wildcard the dotenv namespace. The
   two-file dotenv rule neither bans KSOPS YAML nor authorizes arbitrary additional `.env.enc`
   files — future agents must not infer either.
3. **Release/deployment/bootstrap credentials:** prefer protected workload/environment/KMS/
   secret-manager injection; classify exceptions explicitly rather than adding tracked dotenv
   environments for operational planes.
4. **Evidence hygiene (DEN-2665):** all evidence is value-blind — timestamps, revisions,
   conditions, event types, durations, and error classes only.

## Open decisions

Every subsection below is **OPEN** and lists the DEN-2665 evidence it awaits. Nothing in this
section may be converted to a decision without that evidence attached to DEN-2665 and reviewed.

### D1. Which secret classes, if any, may use SOPS ciphertext in Git

- Status: OPEN.
- Awaits: full Lane A vs Lane B comparison — rotation p50/p95, privileged/manual step counts,
  review clarity, credential scope and namespace/repo blast radius, unexpected diff/cache/log
  exposure — plus hard gates G1, G2, and G8 below.

### D2. Kubernetes SOPS artifact form: KSOPS Secret YAML, canonical encrypted dotenv, or both

- Status: OPEN.
- Awaits: Lane A canary shape evidence from DEN-2662 and DEN-2665 baseline/rotation phases.
- Bounded by fixed constraints 1–2 above: whichever form(s) are approved, the dotenv namespace
  stays exactly two files, and KSOPS YAML is a separately scoped class.

### D3. Approved cluster decrypt identities

Three sub-decisions, each OPEN:

- **D3a. Cloud KMS / workload identity.** Awaits: Lane A KMS/age permission-removal failure
  injection, recovery timing after identity restore, and gates G3/G4/G6.
- **D3b. Per-cluster age identity from an independent recovery root.** Awaits: recipient removal +
  `updatekeys` + data-key rotation offboarding proof, and cold recovery twice from documented
  roots (G6).
- **D3c. Fiducia-held application-only age-key sub-test — rejected, allowed for development, or
  allowed more broadly.** Awaits: that sub-test's rows in the DEN-2665 matrix, including failure
  injection and crossover behavior.

### D4. Laptop-only / shared fleet keys: explicit rejection or limitation

- Status: OPEN (the ticket requires the outcome to be an explicit rejection or a stated
  limitation — silence is not an allowed outcome).
- Awaits: gate G3 ("no original laptop is required for recovery") and the second-operator cold
  recovery runs (G6, G7).

### D5. Which runtime secrets default to Fiducia/ESO

- Status: OPEN.
- Awaits: Lane B rotation p50/p95, ESO refresh behavior, stale-value detection delay,
  false-positive/false-negative alert rates, and the seven-day soak.

### D6. Which bootstrap/recovery secrets must remain in the cloud secret manager

- Status: OPEN.
- Awaits: cold-recovery evidence in both lanes, controller-restart behavior, and the
  Fiducia-unavailable bounded-window results (retained-last-value staleness and alerts).

### D7. Argo CD secret-capable trust surface and AppProject/RBAC requirements

- Status: OPEN.
- Awaits: baseline RBAC/NetworkPolicy/AppProject/pod-security equivalence checks, CMP stop/restart
  behavior, unauthorized-read/decrypt failure proofs (G2), and the prune-safety gate (G5).

### D8. ESO/Fiducia durability, TLS, credential-scope, backup, and alerting prerequisites for any production claim

- Status: OPEN.
- Awaits: Lane B failure matrix — remote-key deny, reader revoke/restore, ESO stop/restart,
  bounded Fiducia outage, deleted-Secret reconciliation — plus the residual-risk list and any
  blocked production prerequisites named by DEN-2665.

### D9. Environment/tenant isolation, recipient offboarding, data-key rotation, application-credential rotation, and historical ciphertext rules

- Status: OPEN.
- Awaits: cross-namespace/repo/environment denial evidence (G2), offboarding and data-key rotation
  runs, crossover results, and soak-period drift observations. Historical-ciphertext handling
  awaits the offboarding/data-key-rotation evidence specifically.

### D10. Promotion/rollback behavior so Git rollback cannot silently restore revoked credentials

- Status: OPEN.
- Awaits: Lane A Git-revision rollback test and gate G8 under the proposed policy.

### D11. AWS/Hetzner portability and disaster-recovery expectations

- Status: OPEN.
- Awaits: portability-effort metric, cold rebuild success evidence, and second-operator
  reproducibility (G6, G7).

## DEN-2665 hard gates — pass/fail record

Every hard gate must be recorded here explicitly before any decision above closes. A failed gate
blocks production approval for the affected lane.

| # | Hard gate (verbatim scope from DEN-2665) | Lane A | Lane B |
|---|---|---|---|
| G1 | Zero plaintext/private-identity leakage | PENDING | PENDING |
| G2 | Cross-namespace/repo/environment reads or decryptions fail | PENDING | PENDING |
| G3 | No original laptop is required for recovery | PENDING | PENDING |
| G4 | Corruption or missing dependencies fail closed | PENDING | PENDING |
| G5 | Automated prune never deletes healthy live resources because a plugin emitted empty/partial output | PENDING | PENDING |
| G6 | Cold recovery succeeds twice per lane from documented roots | PENDING | PENDING |
| G7 | A second operator can follow the runbook without inspecting secret values | PENDING | PENDING |
| G8 | Git rollback cannot silently revive a revoked production credential under the proposed policy | PENDING | PENDING |

PENDING means DEN-2665 has not produced the evidence. No gate may be marked pass/fail from
inference, dry runs, or partial output.

## Classification outcome (to be completed)

To be filled only after the open decisions close. Target shape — one row per secret class, one
authoritative source and recovery root per class, at least two tested recovery paths and
individual offboarding for the chosen key model:

| Secret class | Disposition | Authoritative source | Recovery root | Named blockers (linked) |
|---|---|---|---|---|
| _pending classification_ | OPEN | OPEN | OPEN | OPEN |

## Deliverables tracked by this ADR (placeholders — blocked on decisions)

Deliberately not drafted yet; drafting them now would presuppose decisions.

- [ ] Updated repository/app onboarding contract and example manifests (synthetic placeholders
      only).
- [ ] Key-custody and offboarding runbooks.
- [ ] Rotation and cold-recovery runbooks.
- [ ] CI and policy-enforcement requirements.
- [ ] Small-batch rollout order with named canary repos/services (must begin with
      synthetic/non-production canaries and be reversible).
- [ ] Rollback/no-go criteria.
- [ ] Follow-up issues for any prerequisite not already owned elsewhere.
- [ ] Cross-link closure: DEN-2661 closes only after this ADR is accepted and follow-up ownership
      is complete.

## Non-claims

- This skeleton claims no A/B result, no lane preference, and no production readiness for either
  lane.
- Merged canary manifests or a merged version of this skeleton do not constitute evidence,
  approval, or a live-deployment claim.
- Argo CD trust-surface, ESO, and Fiducia hardening requirements stay owned by their tickets; this
  ADR links them as blockers where the evidence demands it.
