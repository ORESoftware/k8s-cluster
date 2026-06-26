<!-- BEGIN k8s-cluster-submodule-notice -->
> [!NOTE]
> **Canonical source.** This repository is the source of truth for its code. It
> is also vendored as a **secondary** git submodule of
> [ORESoftware/k8s-cluster](https://github.com/ORESoftware/k8s-cluster) at
> `remote/libs` — make changes here, not in that submodule checkout.
>
> On disk: source clone `~/codes/ores/k8s-libs-and-shared-defs` · submodule checkout `~/codes/ores/k8s-cluster/remote/libs`.
<!-- END k8s-cluster-submodule-notice -->

# k8s-libs-and-shared-defs

Shared definitions and small client libraries consumed across the ORESoftware
k8s cluster. This repo was extracted (with history) from `k8s-cluster` at
`remote/libs`, and is consumed back there as a submodule pinned to `main`.

## Contents

| Path | What it is |
|------|------------|
| `pg-defs/` | Canonical Postgres `schema/schema.sql` + generated adapters for every language (rust [sqlx/diesel/sea-orm], typescript [drizzle/typeorm/sequelize/prisma], python [sqlalchemy/django/peewee], go [gorm/bun/ent/sqlc], jvm [jooq/hibernate], dart, gleam, elixir, erlang, ruby [activerecord], php [eloquent/doctrine], csharp [ef-core], fsharp, kotlin [exposed], haskell, ocaml, cpp, zig). Each adapter converts Postgres rows into typed objects/structs. `src/generate.mjs` is the generator. |
| `nats/subject-defs/` | NATS subject definitions + multi-language generator (typescript, javascript, rust, python, gleam, erlang, dart, go, java, haskell, ocaml, fsharp, cpp, zig, elixir). `src/generate.mjs`. |
| `interfaces/redis/`, `interfaces/shared/` | Redis cache shapes and shared interface schemas + generators, at full language parity with nats (typescript, javascript, rust, python, gleam, erlang, dart, go, java, haskell, ocaml, fsharp, cpp, zig, elixir). |
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

## Consumed by k8s-cluster

`k8s-cluster` mounts this repo at `remote/libs` as a submodule pinned to `main`.
After publishing changes here, bump the pin there:
`git submodule update --remote remote/libs && git add remote/libs && git commit`.
See `docs/remote-libs-submodule.md` in k8s-cluster.
