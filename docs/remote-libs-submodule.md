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
> in *the libs repo's* own `.gitmodules` (not this repo's). Any workflow that
> needs the complete shared tree must initialize `remote/libs` recursively or
> async-java will be empty.

## Cloning / checking out

```bash
# fresh clone of k8s-cluster, fully populated:
git clone --recurse-submodules git@github.com:ORESoftware/k8s-cluster.git

# already cloned, or after pulling a commit that bumps the pin:
git submodule update --init --recursive remote/libs
```

A non-recursive checkout can leave nested dependencies empty, which breaks
contracts that traverse the complete shared tree. Rust path dependencies, the
pg-defs checks, and runtime hostPath consumers must always use the exact
superproject gitlink; never replace it with the current head of `main` during CI.

### Authentication boundaries

`k8s-libs-and-shared-defs` is private. Repository CI mints an owner-scoped,
short-lived GitHub App installation token using `K8S_SUBMODULE_APP_ID` and
`K8S_SUBMODULE_APP_PRIVATE_KEY`. The token request is explicitly restricted to
the reviewed repositories required by that job and carries only `contents:read`.
It is revoked after checkout and otherwise expires automatically.

- **Repository checks** — the static-contract job checks out this superproject
  without persisted credentials, then runs the reviewed App-backed helper for
  only `remote/libs`:

  ```bash
  SUBMODULE_REPORT_PATH="$RUNNER_TEMP/static-submodule-access.tsv" \
  K8S_SUBMODULE_APP_ID="<GitHub App ID>" \
  K8S_SUBMODULE_APP_PRIVATE_KEY="<GitHub App private key>" \
    bash scripts/ci/init-submodules-with-github-app.sh remote/libs
  ```

  The helper mints a repository-restricted installation token, recursively
  initializes the exact mode-`160000` gitlink, verifies the checkout SHA against
  the superproject pin, revokes the token, and emits a sanitized report without
  printing credentials.
- **pg-defs checks** — `.github/actions/checkout-remote-libs` resolves the exact
  gitlink SHA, checks out `ORESoftware/k8s-libs-and-shared-defs` at that commit
  with `persist-credentials: false`, and verifies a clean checkout. A caller that
  needs nested repositories must initialize only those reviewed nested paths.
- **Deployment fleet checks** — the backend-contract job uses the same helper to
  mint separate owner-scoped, repository-restricted installation tokens for
  `remote/libs` and `remote/deployments/*`; one owner token is never reused as a
  broad cross-organization credential.
- **Runtime node maintenance** — the node deploy key runs recursive submodule
  initialization, including `remote/libs` → `async-java`.

Do not embed any deploy key, App token, or personal access token in a Git URL,
Git config, workflow log, or artifact. Do not use `--remote` in validation jobs:
consumers must test the exact commit recorded by the cluster superproject.

## Enforced integration contract

The cluster checks both the git plumbing and the generated contracts:

```bash
cd remote/tests
pnpm run test:cli:remote-libs-submodule-contract
pnpm run test:cli:nats-subject-contract
```

The submodule contract locks the canonical repository URL, `main` branch,
gitlink mode and commit, nested `async-java` pin, required shared surfaces, the
resolved Rust/Gleam consumer paths, and the repository-restricted GitHub
App/helper checkout policy. The NATS contract runs the pinned generator in
`--check` mode before comparing every tracked workload subject to the canonical
schema model. Both run in `repo-checks.yml`.

## Bumping the pin (it tracks `main`)

Because the submodule tracks `main`, advancing it to the latest reviewed libs
commit is an explicit source change:

```bash
git submodule update --remote remote/libs   # fast-forwards remote/libs to origin/main
git -C remote/libs submodule update --init --recursive   # refresh nested async-java
git add remote/libs
git commit -m "chore: bump remote/libs submodule to latest main"
```

Committing the moved gitlink (`remote/libs`) is what records the new pin. That
single-path change is also what fires `pg-defs-check.yml`, because pg-defs source
no longer changes inside this repository.

## What changed in this repo when libs was extracted

- `remote/libs` went from 514 tracked files to a single gitlink. History was
  preserved via `git subtree split -P remote/libs` (142 commits) pushed to the
  new repo's `main`.
- The old `submodule "remote/libs/async-java"` entry was **removed** from this
  repo's `.gitmodules`; async-java now lives one level down, inside the libs
  repo's `.gitmodules`.
- Rust/Gleam path dependencies are **unchanged** — the on-disk paths
  (`remote/libs/pg-defs/...`, etc.) are identical once the submodule is checked
  out, so no consumer manifest needed editing.
- Repository checks use the owner-scoped GitHub App helper with recursive pin
  verification and sanitized access reports. The standalone pg-defs workflow
  uses the reusable `.github/actions/checkout-remote-libs` exact-gitlink action,
  and its trigger watches the `remote/libs` gitlink.

## Migrations

The libs repo uses [`dpm` (declarative-postgres-migrate)](https://github.com/declarative-migrations/declarative-postgres-migrate.rs)
for Postgres migrations: `pg-defs/schema/schema.sql` is the declarative source and
`remote/libs/pg-defs/scripts/dpm.sh {diff|verify|review|apply}` converges a live
database onto it with reviewable SQL. See `pg-defs/readme.md` in the libs repo.
The historical caveat about `generate.mjs --check` failing on `smallint` was fixed
upstream; the check passes on the current pin.
