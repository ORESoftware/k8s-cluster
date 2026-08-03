# fiducia-monorepo

Cross-repository coordination point for the Fiducia Cloud product. The application and infrastructure repositories are pinned as Git submodules; cross-cutting production contracts and release gates live here so one service cannot quietly redefine a system-wide promise.

## Managed public beta launch controls

- [Managed public beta service contract v0.1](docs/production/managed-public-beta-service-contract-v0.1.md) — proposed customer and operator contract for DEN-1390.
- [Machine-readable SLO catalog](docs/production/managed-public-beta-slos.json) — exact source-series contracts, objective calculations, alerts, owners, review cadence, readiness state, and evidence.
- [Derived SLO series registry](docs/production/managed-public-beta-slo-derived-series.json) — explicit declarations for query inputs produced by controlled failure/incident harnesses rather than ordinary service scrapes.
- [Managed beta incident runbook](docs/operations/managed-beta-incident-runbook.md) — severity, command, containment, recovery, evidence, and required tabletop procedure.
- [Incident and maintenance communication templates](docs/operations/managed-beta-communication-templates.md) — partner-safe status language and handoff structure.
- [Production safety release gate](docs/security/production-safety-release-gate.md) — threat model, invariants, evidence policy, and route coverage for DEN-1391.
- [Machine-readable gate matrix](docs/security/production-safety-release-gate.json) — required adversarial tests and their current evidence state.
- [Production gate evidence bundle template](docs/security/production-gate-evidence-template.md) — exact-candidate measurements, artifacts, exceptions, and independent sign-off.

Validate the documents, SLO catalog/series, and gate matrix without installing dependencies:

```bash
node tools/validate-production-gates.mjs
node tools/validate-slo-series.mjs
```

A release candidate is not launchable until every SLO source is `measured` with exact-candidate evidence, every required gate row is `passed`, and both stricter checks succeed:

```bash
node tools/validate-production-gates.mjs --require-pass
node tools/validate-slo-series.mjs --require-pass
```

The service contract is an engineering launch proposal until DEN-1390 is independently reviewed and approved. It is not a contractual SLA.
