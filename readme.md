# fiducia-monorepo

Git superproject for the fiducia-cloud repositories.

Each application/service repo is tracked as a git submodule under `apps/`.
The superproject pins each submodule to an exact commit, while `.gitmodules`
sets `branch = main` for every submodule so updates intentionally follow each
repo's main branch.

This repo is the private all-up integration and GitOps view. Individual app
repos keep their own visibility boundaries, so public SDK/protocol repos can
coexist with private control-plane, infra, customer, and runtime repos. See
`docs/repo-boundaries.md`.

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

## Feature Branches

Switch the superproject and every app submodule to the same feature branch:

```sh
scripts/checkout-feature-branch.sh feature/customer-portal-streams
```

If the branch exists on a submodule remote, the script checks it out and
fast-forwards it. If it does not exist yet, the script creates it from
`origin/main`. It refuses dirty superproject or submodule checkouts.

Preview first:

```sh
scripts/checkout-feature-branch.sh feature/customer-portal-streams --dry-run
```

If the feature branch should also become the `.gitmodules` tracking branch for
the superproject branch:

```sh
scripts/checkout-feature-branch.sh feature/customer-portal-streams --set-submodule-branch --stage-pins
```

## Audit

Run the monorepo audit before publishing a deployable pin set:

```sh
scripts/audit-repo-state.sh
```

The audit checks for dirty submodules, stale conflict markers, tracked secret
files, secret-looking values, missing Dockerfiles, Rust runtime images that are
not distroless/nonroot, README app-list drift, and the expected private
visibility of the all-up superproject when `gh` is available.

During local edits, preview the non-dirty checks with:

```sh
scripts/audit-repo-state.sh --allow-dirty
```

## Apps

- `apps/fiducia-admin.rs`
- `apps/fiducia-auth.rs`
- `apps/fiducia-backend.rs`
- `apps/fiducia-brain.rs`
- `apps/fiducia-cli.rs`
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
