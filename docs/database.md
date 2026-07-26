# Database contract

Everything the sync server expects from Postgres: the schema, how it got that
way, and the behaviours that are easy to break by accident.

The schema is small on purpose. The server is zero-knowledge, so most of what a
sync service would normally store simply does not exist here — there is no
column anywhere holding a plaintext OTP seed, a vault key, or a recoverable
password.

## The server never applies DDL

Migrations under `migrations/` are applied by an **operator**, not by the
process at startup (`src/db.rs`). This is deliberate: a failed rollout must not
be able to acquire DDL authority or mutate production schema before a human has
reviewed the change. A pod that starts against an un-migrated database will
fail its readiness probe rather than "helpfully" fixing the schema.

Apply them in filename order. The end-to-end suite proves that applying the
full set **twice** is idempotent, by comparing column, index and constraint
fingerprints between a once- and twice-migrated database — so a re-run during
an uncertain rollout is safe.

## Schema

Everything lives in the `threefa` schema, not `public`. The cluster's Postgres
is shared with other services, and schema isolation is what keeps a careless
`DROP` in another service from reaching these tables.

### `threefa.accounts`

One row per human identity.

| Column | Type | Notes |
|---|---|---|
| `id` | `UUID` PK | Generated server-side. |
| `username` | `TEXT` | Legacy local auth only. Being phased out. |
| `auth_secret` | `TEXT` | Legacy Argon2id PHC verifier. A password cannot be recovered from it. |
| `shared_auth_user_id` | `UUID` | The stable shared-auth subject. The preferred identity. |
| `supabase_user_id` | `UUID` | Supabase compatibility subject. |
| `created_at` | `TIMESTAMPTZ` | |

Two constraints carry real weight:

- **`accounts_identity_present`** — a `CHECK` requiring at least one of
  shared-auth subject, Supabase subject, or the legacy username/secret pair. It
  is what stops an account existing that nobody can ever authenticate as.
- **`accounts_shared_auth_user_idx`** — a *partial* unique index on
  `shared_auth_user_id WHERE shared_auth_user_id IS NOT NULL`. Partial matters:
  a plain `UNIQUE` would treat multiple `NULL`s as distinct in Postgres but
  would still index them, and the intent here is specifically "at most one
  account per shared-auth subject, and legacy accounts are exempt". This index
  is what makes repeated enrollment by the same person land on the *same*
  account rather than silently forking their vault.

### `threefa.devices`

One row per enrolled device. An account may have at most 25
(`MAX_DEVICES_PER_ACCOUNT`, enforced in the handler, not by a constraint).

| Column | Type | Notes |
|---|---|---|
| `id` | `UUID` PK | The `device_id` clients send in a version vector. |
| `account_id` | `UUID` | `REFERENCES accounts(id) ON DELETE CASCADE`. |
| `device_name` | `TEXT` | User-supplied, ≤ 200 chars, validated in the handler. |
| `sync_token_hash` | `TEXT` | **SHA-256 hex of the bearer token. The token itself is never stored.** |
| `revoked` | `BOOLEAN` | Revoked devices are kept, not deleted — see below. |
| `created_at`, `last_seen_at` | `TIMESTAMPTZ` | |

Authentication is an indexed equality lookup on `sync_token_hash`
(`devices_token_idx`). Because the comparison is against a digest rather than
the secret, there is no timing oracle on the raw token, and a database leak
does not yield usable credentials.

**Revoked devices are retained deliberately.** Deleting the row would free the
device to re-enroll into the same `device_id` and would erase the audit trail
of what was once authorised. `GET /v1/devices` returns revoked devices with
`revoked: true` so the owner can see them.

### `threefa.vault_blobs`

Exactly one row per account — `account_id` is itself the primary key.

| Column | Type | Notes |
|---|---|---|
| `account_id` | `UUID` PK | `ON DELETE CASCADE`. |
| `ciphertext` | `TEXT` | Base64. Opaque. |
| `nonce` | `TEXT` | Base64. 24 bytes decoded (XChaCha20). |
| `kdf_salt` | `TEXT` | Base64. |
| `kdf_params` | `JSONB` | Argon2id parameters — public by design, since another device must reproduce the key. |
| `version` | `JSONB` | The `VersionVector`. |
| `updated_at` | `TIMESTAMPTZ` | Not advanced by a rejected push. |

**The at-rest encoding is not the wire encoding.** On the wire these three
fields are JSON arrays of integers, because that is what `serde` emits for
`Vec<u8>`. At rest they are base64 `TEXT`, converted from the original `BYTEA`
by migration `0003`, so that the shared pg-defs adapters give every generated
language binding the same lossless contract. Anyone reading a row by hand, or
writing a tool against the table, needs to know these differ.

## Migration history, and why it looks the way it does

| File | What it did |
|---|---|
| `0001_init.sql` | Original schema in `public`, with `BYTEA` payload columns and username/Argon2id auth. |
| `0002_isolate_threefa_schema.sql` | Moved the tables into the `threefa` schema. |
| `0003_supabase_auth.sql` | Added Supabase identity; converted the payload columns to base64 `TEXT`. |
| `0004_shared_auth_identity.sql` | Added `shared_auth_user_id` with its partial unique index; replaced the identity `CHECK` so shared-auth can represent providers that have no Supabase subject. |

`0002` **moves** tables rather than editing `0001`, and the reason is worth
preserving: migration runners record a checksum per file, so editing a file
that has already been applied makes the next legitimate upgrade fail. Later
migrations are similarly written to be re-runnable — guarded by
`IF NOT EXISTS`, `to_regclass` checks, and `information_schema` probes for the
column type — which is what makes double application safe.

## Behaviours that are easy to break

**The push path locks the `accounts` row, not the blob row.** `POST /v1/vault`
opens a transaction and takes `LockType::Update` on `accounts` before reading
`vault_blobs`. Locking the account rather than the blob is deliberate and easy
to "simplify" wrongly: on an account's *first* push there is no `vault_blobs`
row to lock, so two concurrent first pushes would both see nothing, both
insert, and one would lose. The account row always exists, so it is the only
thing that can serialise that case.

This lock is what makes the version-vector conflict check sound at all —
without it two devices could read the same base version and both believe they
won. It also means a slow push blocks that account's other pushes, which is one
reason the authenticated routes carry their own rate limit.

**The connection pool is 10.** `src/db.rs` caps `max_connections(10)` with a
5-second acquire timeout and `test_before_acquire(true)`. Under contention the
pool is the first thing to saturate, and an exhausted pool surfaces as an
acquire timeout, which now maps to **503** rather than 500 — that mapping is
what lets callers tell "retry shortly" from "this deployment is broken".

**Readiness pings through the instrumented path.** `/readyz` runs its `SELECT 1`
via the same execution path as real queries, so it moves the query counter.
It previously bypassed it, which made the single most frequent database
operation in a live deployment invisible to metrics.

**Nothing here is a substitute for the handler's validation.** Size and shape
limits — ciphertext length, nonce length, version-vector well-formedness, the
device cap — are enforced in Rust before the transaction opens, not by
constraints. If you add a write path, it must go through the same validators;
the schema will not catch a malformed blob for you.
