# canonical-interfaces

JSON Schema (typed-IO) source of truth for the **canonical.cloud** HTTP API
(served by [`canonical-web-server.rs`](https://github.com/canonical-cloud/canonical-web-server.rs)),
including its bounded offline-first `draft_note` sync protocol, plus a
Supabase Postgres reference schema and generated language adapters. Same spirit as
[`akrion-sim-interfaces`](https://github.com/akrion-sim/akrion-sim-interfaces)
and [`sonus-auris-interfaces`](https://github.com/sonus-auris/sonus-auris-interfaces).

JSON Schema (`schema/*.schema.json`, indexed by `schema/index.json`) is the
single source of truth. API definitions use the exact camelCase JSON wire names;
the compliance-domain schema retains its established snake_case names.
Everything under `generated/` is an **adapter** — never hand-edit it; add a type
as a `$def` and regenerate.

| Language | Path |
| --- | --- |
| TypeScript | `generated/typescript/index.ts` |
| Rust (serde) | `generated/rust/src/lib.rs` |
| Rust → WebAssembly (tsify) | `generated/rust-wasm/src/lib.rs` |
| Python (dataclasses) | `generated/python/canonical_interfaces.py` |
| Go | `generated/go/interfaces.go` |

The `rust-wasm` target is the same serde types as `rust` plus
[`tsify`](https://github.com/madonoharu/tsify) + `wasm-bindgen`, so payloads cross
the JS/wasm boundary as real objects (with an emitted `.d.ts`). It is a separate
crate so the plain `rust` crate stays dependency-free. Build it with
`wasm-pack build generated/rust-wasm --target web`.

## Types

The web server's HTTP contract (`schema/api.schema.json`):

- **`HealthStatus`** — legacy-compatible response of `GET /api/health` and
  `GET /api/v1/health` (`{ status, service }`).
- **`ServiceInfo`** — response of `GET /api/info` and `GET /api/v1/info`,
  including the reported sMASH `stack`.
- **`DraftNoteValue`**, **`DraftNoteKey`**, and **`WireRecord`** — schema-v1
  draft-note values, owner-scoped keys, authoritative snapshots, and tombstones.
- **`MutationOperation`**, **`MutationRequest`**, **`MutationResult`**, and
  **`MutationResponse`** — the idempotent compare-and-swap contract for
  `POST /api/v1/sync/mutations`.
- **`ChangesQuery`** and **`ChangesResponse`** — bounded incremental pulls from
  `GET /api/v1/sync/changes` using an opaque owner-bound cursor.

The v1 sync contract accepts only `draft_note`, only payload schema version 1,
1–50 operations per mutation request, titles up to 200 characters, bodies up to
100,000 characters, and pull pages up to 500 records. Record versions are JSON
decimal strings, not JavaScript numbers. Mutation results are `applied`,
`conflict`, `gone`, `invalid`, or `idempotency_key_reused`. REST pull is
authoritative; WebSocket invalidations only wake the pull loop.

The compliance domain (`schema/compliance.schema.json`, mirrored by
`sql/schema.sql`):

- **`AuditEngagement`** — a customer compliance-audit engagement (framework +
  lifecycle status).

## Postgres safety contract

`sql/schema.sql` documents the owner-aware `user_profile`, `sync_clock`,
`sync_record`, `sync_change`, and `sync_receipt` shapes used by the web server,
the owner-scoped compliance tables, and the server-only `web_session` shape.
The executable SeaORM migration in the web-server repository remains the
runtime migration.

Owner-scoped tables enable and force RLS with `auth.uid()` policies. The web
server must set verified `request.jwt.claims` locally inside every user
transaction and lock the owner's `sync_clock` row while it mutates the record,
advances the clock, appends the change, and writes the idempotency receipt. That
single transaction gives pull cursors commit order without gaps. The browser
receives neither database credentials nor Supabase token pairs. The shared SQL
file names only the encrypted-at-rest credential columns so schema drift is
detectable; `web_session` is absent from generated wire adapters and customer
database grants. Its forced-RLS process policy permits the customer web server,
or the no-ingress revocation worker only while its transaction-local
`canonical.system_task = session_revocation` marker is present. Retry,
abandonment, and failure-kind columns distinguish pending upstream sign-out
from terminal dead-letter outcomes without weakening immediate local logout.
Nullable refresh-lease identity and expiry columns fence token rotation across
server replicas without keeping a database transaction open during an upstream
authentication request.
The custom setting is an audit/accidental-code-path guard, not a secret
capability; authorization rests on the exact isolated revoker login and its
one-table grant.

Administrative authorization is a separate data plane. The SQL reference also
defines bounded `admin_role_assignment` rows and append-only
`admin_audit_event` evidence, but deliberately adds no customer RLS policy or
customer grant for either table. A future separately deployed admin server uses
an independent non-`BYPASSRLS` database role and only two reviewed
`SECURITY DEFINER` functions: capability lookup and audit append. Owner policies
never grow a generic `OR is_admin` escape hatch, and no admin wire API is
generated until concrete endpoints exist. Both functions bind the actor to
`auth.uid()` from a server-verified, transaction-local claims context and reject
privileged access unless the Supabase `aal` claim is `aal2`; callers cannot pass
an arbitrary actor ID.

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

1. Add a PascalCase `$def` to a `schema/*.schema.json` file. Use exact
   lowerCamelCase properties for JSON API wire fields and snake_case for the
   existing compliance domain (or add a new file and list it in
   `schema/index.json`).
2. `npm run generate` and commit the regenerated `generated/` output.
3. If it's a stored entity, mirror it in `sql/schema.sql`.
