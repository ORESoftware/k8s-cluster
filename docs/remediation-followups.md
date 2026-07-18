# Remediation & follow-ups

Notes from the 2026-07-17/18 security + hygiene hardening pass on this repo:
what was fixed, what is still open, and how to shore each item up. Ordered by
priority within each section. The "Fixed" list is context so a reader knows the
current baseline; the actionable work is under **Open**.

## Fixed in this pass (baseline)

Committed on `main` (tip `8c36930` and its ancestors):

- **Credential exposure**
  - `cli-config-client-gleam` `snapshot_json/0` no longer serialises the entire
    process environment — it now lists only flags declared in `.cli-flags.toml`
    and redacts secret-named keys. (It falls back to plain env vars for lookups,
    so the old snapshot leaked every `DATABASE_URL`/`*_SECRET` the pod held.)
  - `runtime-config-client-rs` and `runtime-config-client-gleam`: `GET
    /internal/runtime-config` now requires `X-Server-Auth` whenever a push
    secret is configured (the snapshot lists every pushed entry value).
  - `pg-defs/src/diff.mjs`: the Postgres password is passed via `PGPASSWORD`,
    not on the `psql` argv (which is world-readable via `ps`/procfs); the URL
    scheme is validated and goes through `--dbname`.
  - `wal-consumer-rs`: decode errors log payload length, not payload bytes.
- **Transport / supply chain**
  - `diff.mjs` defaults `PGSSLMODE=require` for non-loopback hosts with no
    explicit `sslmode` (libpq's `prefer` silently downgrades to plaintext).
  - `.github/workflows/ci.yml`: actions SHA-pinned; the `dpm` installer is
    pinned to a reviewed commit and fetched with `--proto '=https'` (was
    `curl | bash` off `main`).
- **Codegen injection hardening** (inputs are in-repo/trusted; defense-in-depth)
  - `nats` / `redis` / `shared` generators gate every schema name that becomes a
    code identifier against `^[A-Za-z_][A-Za-z0-9_]*$`.
  - `splitDoc` in all three neutralises `*/` and strips control chars so block
    comments can't be terminated early.
  - `pg-defs/src/generate.mjs`: output paths are asserted to stay under
    `generated/`; `rustRawString` grows its `#` fence dynamically;
    `escapeTemplate` escapes backslashes first.
  - `runtime-config-client-gleam` FFI: full JSON string escaping (was
    quote-only).
- **Hygiene**: `.gitignore` now covers `.env`/`*.env`/`env/`; `telemetry-node`
  `pnpm-lock.yaml` re-synced with `package.json`; `manifest.toml` lockfiles
  committed for both previously-unpinned Gleam packages.

## Open — high value

### 1. No CI coverage for the Rust / Go / Gleam client libs
`ci.yml` runs only the Node generators (`--check` + `*.test.mjs`) and the
schema-migration verify. The Rust crates, `telemetry-go`, and the three Gleam
packages are **not built or tested in CI**, so a regression in any of them
(including the auth/redaction changes above) ships unnoticed.

**Shore up:** add jobs (or a matrix) that run:
- `cargo test` in `runtime-config-client-rs`, `wal-consumer-rs`, `telemetry-rs`,
  and `pg-defs/generated/rust` (build only for the generated crate).
- `go build ./... && go vet ./... && go test ./...` in `telemetry-go`.
- `gleam build` (and `gleam test` where suites exist) for the three Gleam
  packages. Note the OTP-29 caveat in item 4 — the Gleam jobs need
  `ERL_COMPILER_OPTIONS=[nowarn_deprecated_catch]` until that's resolved.

### 2. Thin test coverage on the security-relevant changes
The auth/redaction/validation logic added this pass is largely untested:
- **`runtime-config-client-rs` has zero tests.** Add axum handler tests:
  `handle_get` returns 401 without `X-Server-Auth` when a secret is set, 200
  with the correct header, and stays open when no secret is configured; confirm
  `constant_time_eq` rejects length mismatches.
- **`wal-consumer-rs::assert_subject_token`** has no negative test. Add cases
  asserting it panics for tokens containing `.`, `*`, `>`, whitespace, or empty,
  and passes for `[A-Za-z0-9_-]`.
- **Gleam FFI** (`dd_runtime_config_client_ffi`, `dd_cli_config_client_ffi`):
  add gleeunit/eunit tests for `escape_string` (backslash, quote, newline,
  control char), `read_auth_ok` (open when unset, constant-time when set),
  `secret_key` redaction, and that `snapshot_json` lists only declared flags.

## Open — medium

### 3. `pg-defs` package-manager mismatch
`pg-defs/package.json` declares `"packageManager": "pnpm@9.15.4"` but the repo
commits an npm `package-lock.json` (every other pnpm package here commits a
`pnpm-lock.yaml` or nothing). Divergent resolvers can produce different trees.

**Shore up:** pick one. Recommended: standardise on pnpm — delete
`pg-defs/package-lock.json`, run `pnpm install` to generate/commit a
`pnpm-lock.yaml`, and align the declared `packageManager` version with the one
used repo-wide (`pnpm@10.33.0`).

### 4. `otel-client-gleam` does not build under OTP 27+
Under OTP 29 (the local toolchain), a transitive HTTP/2 dependency of
`opentelemetry_exporter` (`chatterbox`, module `h2_stream_set.erl`) uses the
deprecated `catch Expr` form, which newer OTP treats as a hard compile error.
The package's own code is fine; only the dep fails.

**Workaround (verified):** `ERL_COMPILER_OPTIONS="[nowarn_deprecated_catch]"
gleam build` compiles cleanly. A project-level `rebar.config` does **not** fix
it — Gleam orchestrates the per-dependency build and bypasses rebar3's
`overrides`.

**Shore up (pick one):**
- Set `ERL_COMPILER_OPTIONS=[nowarn_deprecated_catch]` in the CI Gleam job and
  document it in the package README (fastest; unblocks builds today).
- Bump `opentelemetry_exporter` (and its `chatterbox`/`grpcbox` closure) to a
  release cut for newer OTP, then drop the flag.
- Pin the build/runtime image to OTP < 29 until upstream catches up.

Recommend the flag + a tracking note, and revisit when the exporter updates.

## Open — low / defense-in-depth

### 5. `diff.mjs` resolves `psql` from `$PATH`
`spawn("psql", …)` trusts `$PATH`; a malicious `psql` earlier on the path would
run. Exposure is local-only and this is an operator tool, so it's low priority.
**Shore up:** allow an absolute `psql` path via env (e.g. `PSQL_BIN`) and/or
verify the binary before spawning.

### 6. Deprecated transitive `glob@10.5.0`
Pinned via the `typeorm` peer closure in `pg-defs/package-lock.json` (all such
entries are `"peer": true`, so it's not a direct runtime dep), but it surfaces
in any `npm audit`. **Shore up:** refreshes out when `typeorm` updates its
`glob` range; nothing to do directly. Resolving item 3 (drop the npm lockfile)
removes it from this repo's `audit` surface entirely.

### 7. Prototype-pollution defense-in-depth in the generators
No exploitable sink exists today (schema inputs are trusted; no recursive
untrusted-object merge). If untrusted schema sources ever become possible,
build the schema-derived lookup objects with `Object.create(null)` or reject
`__proto__`/`constructor`/`prototype` keys when iterating `def.properties`.

## How to verify the current baseline

```sh
# Node generators + tests (fast, no deps)
node pg-defs/src/generate.mjs --check
node nats/subject-defs/src/generate.mjs --check
node interfaces/redis/src/generate.mjs --check
node interfaces/shared/src/generate.mjs --check
node --test pg-defs/src/*.test.mjs interfaces/*/src/*.test.mjs nats/subject-defs/src/*.test.mjs
node interfaces/shared/src/validate-examples.mjs
node pg-defs/src/diff.mjs --parse-only

# Rust
for c in runtime-config-client-rs wal-consumer-rs telemetry-rs; do (cd "$c" && cargo test); done

# Go
(cd telemetry-go && go build ./... && go vet ./...)

# Gleam (see item 4 for the OTP-29 flag)
for g in runtime-config-client-gleam cli-config-client-gleam; do (cd "$g" && gleam build); done
ERL_COMPILER_OPTIONS="[nowarn_deprecated_catch]" sh -c 'cd otel-client-gleam && gleam build'
```
