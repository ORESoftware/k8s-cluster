# `remote/libs` submodule (k8s-libs-and-shared-defs)

`remote/libs` is a **git submodule**, not plain files in this repo. It points at
`git@github.com:ORESoftware/k8s-libs-and-shared-defs.git` and is **pinned to the
`main` branch**. It holds the shared definitions consumed across the cluster:

```
remote/libs/
├── async-java/                  # nested submodule -> async-java/async.java.git (branch master)
├── browser/
├── cli-config-client-gleam/
├── interfaces/                  # redis + shared interface schemas + generators
├── nats/                        # subject-defs + generators
├── pg-defs/                     # canonical schema.sql + multi-language generated adapters
├── runtime-config-client-gleam/
├── runtime-config-client-rs/
└── wal-consumer-rs/
```

> **Nested submodule:** `remote/libs/async-java` is itself a submodule, declared
> in *the libs repo's* own `.gitmodules` (not this repo's). Any clone/update of
> `remote/libs` must therefore be **recursive** or async-java will be empty.

## Cloning / checking out

```bash
# fresh clone of k8s-cluster, fully populated:
git clone --recurse-submodules git@github.com:ORESoftware/k8s-cluster.git

# already cloned, or after pulling a commit that bumps the pin:
git submodule update --init --recursive remote/libs
```

A non-recursive checkout leaves `remote/libs` empty, which breaks Rust path-deps
(`Cargo.toml` `path = "../../libs/..."`), the pg-defs CI, and the runtime node's
hostPath mounts. Always recurse.

### Auth

`k8s-libs-and-shared-defs` is a **private ORESoftware repo** (same as the other
private submodules: `live-mutex`, `sonus-auris-backend.rs`, `3fa-backend`, …),
so anything that checks it out needs ORESoftware-scoped credentials:

- **GitHub Actions** — `actions/checkout@v4` with `submodules: recursive`
  (see `repo-checks.yml` and `pg-defs-check.yml`). Uses the default token, the
  same mechanism the existing private submodules already rely on.
- **Runtime node** — `remote-k8s-maintenance.yml` runs
  `git submodule update --init --recursive` with the node's deploy key; this
  recurses into `remote/libs` → `async-java`.

## Bumping the pin (it tracks `main`)

Because the submodule tracks `main`, advancing it to the latest published libs:

```bash
git submodule update --remote remote/libs   # fast-forwards remote/libs to origin/main
git -C remote/libs submodule update --init --recursive   # refresh nested async-java
git add remote/libs
git commit -m "chore: bump remote/libs submodule to latest main"
```

Committing the moved gitlink (`remote/libs`) is what records the new pin. That
single-path change is also what fires `pg-defs-check.yml` (its trigger watches
`remote/libs`, since pg-defs source no longer changes inside this repo).

## What changed in this repo when libs was extracted

- `remote/libs` went from 514 tracked files to a single gitlink. History was
  preserved via `git subtree split -P remote/libs` (142 commits) pushed to the
  new repo's `main`.
- The old `submodule "remote/libs/async-java"` entry was **removed** from this
  repo's `.gitmodules`; async-java now lives one level down, inside the libs
  repo's `.gitmodules`.
- Rust/Gleam path-deps are **unchanged** — the on-disk paths
  (`remote/libs/pg-defs/...`, etc.) are identical once the submodule is checked
  out, so no consumer manifest needed editing.
- `pg-defs-check.yml` checkouts gained `submodules: recursive` and its trigger
  was repointed from `remote/libs/pg-defs/**` to the `remote/libs` gitlink.

## Migrations

The libs repo now uses [`dpm` (declarative-postgres-migrate)](https://github.com/declarative-migrations/declarative-postgres-migrate.rs)
for Postgres migrations: `pg-defs/schema/schema.sql` is the declarative source and
`remote/libs/pg-defs/scripts/dpm.sh {diff|verify|review|apply}` converges a live
database onto it with reviewable SQL. See `pg-defs/readme.md` in the libs repo.
The historical caveat about `generate.mjs --check` failing on `smallint` was fixed
upstream; the check passes on the current pin.
