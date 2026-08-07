# Benefactor foundation repositories

This runbook owns the idempotent bootstrap for the Benefactor package layers that sit between canonical interfaces and the existing automation, sync, E2E, HubSpot, Postgres/RDS, Gmail, and SendGrid surfaces.

## Repository graph

```text
benefactor-interfaces
        ↓
  benefactor-lib
        ↓
benefactor-clients
        ↓
benefactor-cli ──→ oresoftware/flags-2-env
        ↓
 benefactor-gas
```

`benefactor-gas` also imports the interfaces, library, and clients packages directly. Every edge is declared in `.zpkg.toml`; `.zpkg.lock` is present in each repository. Rust is the primary domain/runtime language, TypeScript mirrors browser and Node.js boundaries, and `benefactor-lib` exposes a `wasm-pack` build for web and Node targets.

## Publication boundary

The GitHub app used by interactive automation cannot create organization repositories. The reviewed workflow therefore uses the established AWS OIDC → SSM → protected GitHub profile path. The protected token is resolved only on the SSM host, validated as the `ORESoftware` identity, and never returned to the Actions runner. The publisher independently verifies active admin membership in `benefactor-cc`.

The publisher has an exact four-repository creation allowlist. It preserves `benefactor-interfaces`, never overwrites an existing repository, creates missing repositories as private, initializes `main` through the Git Data API, and verifies every remote ref before emitting non-secret evidence.

## Contact-discovery and outreach boundary

The repository seeds provide policy logic, typed ports, a fail-closed CLI, and a bounded Apps Script approval gateway. They do not run browser scraping or send mail during bootstrap. Live implementation must retain source provenance, respect robots/provider terms and access controls, reconcile HubSpot and the `benefactor` Postgres namespace, apply suppression/unsubscribe checks, use idempotency keys, and require a reviewed campaign identifier before Gmail or SendGrid handoff.

The intended operational sequence is:

1. Read ICP profiles from the `benefactor` Postgres namespace and create Brave/Serp query plans.
2. Enqueue approved browser jobs in `ORESoftware/k8s-cluster` with recoverable job state and bounded concurrency.
3. Normalize and deduplicate business contacts, retaining source evidence and collection timestamps.
4. Upsert eligible contacts into AWS RDS and HubSpot; route uncertain or personal-contact records to review rather than guessing.
5. Reconcile suppression, unsubscribe, and prior-attempt state.
6. Preview a 200–300-contact campaign, approve a campaign ID, then send in smaller rate-limited batches through Gmail or SendGrid with delivery evidence.

## Project routing

- GitHub Project: `https://github.com/orgs/benefactor-cc/projects/1`
- Linear project: `github.com/benefactor-cc` in the Denman workspace
- GitHub is authoritative for code, pull requests, CI, releases, and runtime evidence.
- Linear is authoritative for product scope, ownership, dependencies, milestones, and status.
