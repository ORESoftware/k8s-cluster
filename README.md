# k8s-libs-and-shared-defs

Shared definitions and small client libraries consumed across the ORESoftware
k8s cluster. This repo was extracted (with history) from `k8s-cluster` at
`remote/libs`, and is consumed back there as a submodule pinned to `main`.

## Contents

| Path | What it is |
|------|------------|
| `pg-defs/` | Canonical Postgres `schema/schema.sql` + generated adapters for every language (rust, gleam, typescript, python, go, elixir, erlang, jvm, dart, prisma, drizzle, …). `src/generate.mjs` is the generator. |
| `nats/subject-defs/` | NATS subject definitions + multi-language generator (`src/generate.mjs`). |
| `interfaces/redis/`, `interfaces/shared/` | Redis cache shapes and shared interface schemas + generators. |
| `runtime-config-client-rs/`, `runtime-config-client-gleam/`, `cli-config-client-gleam/` | Runtime/CLI config client libs. |
| `wal-consumer-rs/` | WAL consumer lib. |
| `browser/` | Browser helper (service worker). |
| `async-java/` | **Nested submodule** → `async-java/async.java.git` (branch `master`). |

## Cloning

`async-java` is a nested submodule, so clone recursively:

```bash
git clone --recurse-submodules git@github.com:ORESoftware/k8s-libs-and-shared-defs.git
# or, in an existing checkout:
git submodule update --init --recursive
```

## Generators

The generators are **Node-stdlib-only** (no `npm install` needed). Each supports
a `--check` mode (regenerate in memory, fail if committed output drifted):

```bash
node pg-defs/src/generate.mjs --check
node nats/subject-defs/src/generate.mjs --check
node interfaces/redis/src/generate.mjs --check
node interfaces/shared/src/generate.mjs --check
```

To regenerate after editing a schema, run the same command without `--check` and
commit the updated `generated/` files.

> **Known issue:** `pg-defs/src/generate.mjs --check` currently throws
> `Unsupported SQL type for Drizzle: smallint` — `drizzleColumn()` in
> `pg-defs/src/generate.mjs` lacks a `smallint` case while `schema.sql` has a
> `battery_level smallint` column. Add the mapping (and its drizzle-orm import)
> before relying on the pg-defs drift check.

## Consumed by k8s-cluster

`k8s-cluster` mounts this repo at `remote/libs` as a submodule pinned to `main`.
After publishing changes here, bump the pin there:
`git submodule update --remote remote/libs && git add remote/libs && git commit`.
See `docs/remote-libs-submodule.md` in k8s-cluster.
