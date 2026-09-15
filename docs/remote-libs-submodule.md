# `remote/libs` submodule (k8s-libs-and-shared-defs)

`remote/libs` is a **git submodule**, not copied source. It points at
`git@github.com:ORESoftware/k8s-libs-and-shared-defs.git`, tracks `main`, and is
pinned by the cluster superproject to one immutable commit.

```text
remote/libs/
├── async-java/                  # public nested submodule
├── browser/
├── cli-config-client-gleam/
├── interfaces/
├── nats/
├── pg-defs/
├── runtime-config-client-gleam/
├── runtime-config-client-rs/
└── wal-consumer-rs/
```

`remote/libs/async-java` is declared by the library repository's own
`.gitmodules`. Any workflow that needs the complete shared tree must initialize
`remote/libs` recursively.

## Local checkout

```bash
git clone --recurse-submodules git@github.com:ORESoftware/k8s-cluster.git
git submodule update --init --recursive remote/libs
```

Never use `--remote` in validation jobs. CI must test the exact mode-`160000`
gitlink recorded by the cluster commit, not whichever commit is currently at a
branch head.

## CI authentication boundary

Repository checks no longer use `K8S_LIBS_DEPLOY_KEY`. Both the static-contract
and private-deployment jobs now:

1. check out the cluster source without private submodules or persisted
   credentials;
2. resolve the exact `remote/libs` gitlink with `git ls-files --stage`;
3. use `actions/create-github-app-token` with `K8S_SUBMODULE_APP_ID` and
   `K8S_SUBMODULE_APP_PRIVATE_KEY`;
4. request a token restricted to
   `ORESoftware/k8s-libs-and-shared-defs` with only `contents:read`;
5. check out that exact gitlink recursively at `remote/libs`; and
6. verify the checked-out HEAD equals the superproject pin.

The same reviewed App credential pair is used for private deployment gitlinks,
but each runtime token is owner- and repository-restricted. The authoritative
repository set is
`config/ci/k8s-submodule-github-app-allowlist.json`; it is the exact union of
`remote/libs` and the private `remote/deployments/*` gitlinks.

The App private key is never embedded in a Git URL, Git config, workflow log, or
artifact. Installation tokens are short-lived and revoked by their owning
workflow/action. A personal access token supplied in chat is not a CI credential
source.

The reusable `.github/actions/checkout-remote-libs` action may still be used by
focused pg-defs workflows. It must resolve the same gitlink, check out the exact
commit with `persist-credentials: false`, and initialize only explicitly
reviewed nested repositories.

## Enforced integration contract

```bash
cd remote/tests
pnpm run test:cli:remote-libs-submodule-contract
pnpm run test:cli:nats-subject-contract
```

The submodule contract locks:

- the canonical repository URL and tracked branch;
- gitlink mode and exact checkout commit;
- recursive `async-java` initialization;
- required shared contract surfaces;
- Rust and Gleam path-dependency resolution;
- repository-restricted App token creation; and
- the absence of the obsolete deploy-key path.

The NATS contract runs the pinned generator in check mode before comparing
tracked workload subjects with the canonical schema.

## Bumping the pin

Advancing the library is an explicit source change:

```bash
git submodule update --remote remote/libs
git -C remote/libs submodule update --init --recursive
git add remote/libs
git commit -m "chore: bump remote/libs submodule to latest main"
```

Review the new library commit before recording the moved gitlink. A validation
or packaging job must never perform this update implicitly.

## Extraction history

- `remote/libs` moved from copied files to one gitlink while preserving its
  upstream history.
- `remote/libs/async-java` moved into the library repository's `.gitmodules`.
- Existing Rust and Gleam on-disk dependency paths remained unchanged.
- Repository checks now use the same fail-closed, repository-restricted GitHub
  App model as other private source checks.

## Database migration boundary

The library repository uses `dpm` (`declarative-postgres-migrate`) for Postgres
migrations. `pg-defs/schema/schema.sql` is the declarative source, and
`remote/libs/pg-defs/scripts/dpm.sh {diff|verify|review|apply}` produces
reviewable convergence SQL. Migration application remains human reviewed.
