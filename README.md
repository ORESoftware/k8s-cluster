# fiducia-monorepo

Cross-repository coordination point for the Fiducia Cloud product. The application and infrastructure repositories are pinned as Git submodules; cross-cutting production contracts and release gates live here so one service cannot quietly redefine a system-wide promise.

## Managed public beta launch controls

- [Managed public beta service contract v0.1](docs/production/managed-public-beta-service-contract-v0.1.md) — proposed customer and operator contract for DEN-1390.
- [Production safety release gate](docs/security/production-safety-release-gate.md) — threat model, invariants, evidence policy, and route coverage for DEN-1391.
- [Machine-readable gate matrix](docs/security/production-safety-release-gate.json) — required adversarial tests and their current evidence state.

Validate the documents and matrix without installing dependencies:

```bash
node tools/validate-production-gates.mjs
```

A release candidate is not launchable until the evidence bundle has updated every required matrix row to `passed` and this stricter command succeeds:

```bash
node tools/validate-production-gates.mjs --require-pass
```

The service contract is an engineering launch proposal until DEN-1390 is independently reviewed and approved. It is not a contractual SLA.
