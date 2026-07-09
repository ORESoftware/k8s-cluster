# canonical-interfaces

JSON Schema (typed-IO) source of truth for the **canonical.cloud** HTTP API
(served by [`canonical-backend.rs`](https://github.com/canonical-cloud/canonical-backend.rs)),
plus the canonical Postgres schema for its compliance store, with generated
language adapters. Same spirit as
[`akrion-sim-interfaces`](https://github.com/akrion-sim/akrion-sim-interfaces)
and [`sonus-auris-interfaces`](https://github.com/sonus-auris/sonus-auris-interfaces).

JSON Schema (`schema/*.schema.json`, indexed by `schema/index.json`) is the
single source of truth. Everything under `generated/` is an **adapter** — never
hand-edit it; add a type as a `$def` and regenerate.

| Language | Path |
| --- | --- |
| TypeScript | `generated/typescript/index.ts` |
| Rust (serde) | `generated/rust/src/lib.rs` |
| Python (dataclasses) | `generated/python/canonical_interfaces.py` |
| Go | `generated/go/interfaces.go` |

## Types

The backend's HTTP contract (`schema/api.schema.json`):

- **`HealthStatus`** — response of `GET /api/health` (`{ status, service }`).
- **`ServiceInfo`** — response of `GET /api/info` (`{ service, version, domain }`).

The compliance domain (`schema/compliance.schema.json`, mirrored by
`sql/schema.sql`):

- **`AuditEngagement`** — a customer compliance-audit engagement (framework +
  lifecycle status).

## Use

```sh
npm install
npm run generate     # regenerate generated/<lang> from schema/
npm run check        # verify generated/ is up to date (CI)
npm test             # generator self-tests + --check
```

Consume the generated adapters via the package `exports` map, e.g.
`@canonical-cloud/interfaces/typescript`, `.../rust`, `.../sql`.

## Add a type

1. Add a PascalCase `$def` (snake_case fields) to a `schema/*.schema.json` file
   (or add a new file and list it in `schema/index.json`).
2. `npm run generate` and commit the regenerated `generated/` output.
3. If it's a stored entity, mirror it in `sql/schema.sql`.
