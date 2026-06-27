# fiducia-monorepo

Git superproject for the fiducia-cloud repositories.

Each application/service repo is tracked as a git submodule under `apps/`.
The superproject pins each submodule to an exact commit, while `.gitmodules`
sets `branch = main` for every submodule so updates intentionally follow each
repo's main branch.

## Clone

```sh
git clone --recurse-submodules git@github.com:fiducia-cloud/fiducia-monorepo.git
```

For an existing checkout:

```sh
git submodule update --init --recursive
```

## Update Pins

```sh
scripts/pin-submodules.sh main
git status
git diff --cached --submodule
git commit -m "Pin Fiducia apps to main"
```

To pin all submodules to another branch, use the branch name:

```sh
scripts/pin-submodules.sh dev
git commit -m "Pin Fiducia apps to dev"
```

The script verifies that the target branch exists on every submodule remote,
refuses dirty submodule checkouts, updates every `.gitmodules` `branch` entry,
fast-forwards each submodule, and stages the resulting gitlink pins.

Preview without changing files:

```sh
scripts/pin-submodules.sh dev --dry-run
```

## Apps

- `apps/fiducia-admin.rs`
- `apps/fiducia-auth.rs`
- `apps/fiducia-backend.rs`
- `apps/fiducia-brain.rs`
- `apps/fiducia-clients`
- `apps/fiducia-customer-ui.web`
- `apps/fiducia-edge`
- `apps/fiducia-infra`
- `apps/fiducia-interfaces`
- `apps/fiducia-load-balance.rs`
- `apps/fiducia-node-sidecar.rs`
- `apps/fiducia-node.rs`
- `apps/fiducia-routing.rs`
- `apps/fiducia-telemetry.rs`
- `apps/fiducia-ui.web`
