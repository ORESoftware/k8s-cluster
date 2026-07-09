# canonical-monorepo

Git superproject for the [canonical-cloud](https://github.com/canonical-cloud)
repositories.

Each application/service repo is tracked as a git **submodule** under `apps/`.
The superproject pins each submodule to an exact commit, while `.gitmodules`
sets `branch = main` for every submodule so updates intentionally follow each
repo's main branch.

## Apps

| Submodule                    | Stack        | Repo |
| ---------------------------- | ------------ | ---- |
| `apps/canonical-backend.rs`  | Rust / axum  | [canonical-backend.rs](https://github.com/canonical-cloud/canonical-backend.rs) |
| `apps/canonical-frontend`    | Astro        | [canonical-frontend](https://github.com/canonical-cloud/canonical-frontend) |

`canonical-frontend` builds the static site; `canonical-backend.rs` serves it
(plus a small JSON API). See `docs/repo-boundaries.md`.

## Clone

```sh
git clone --recurse-submodules git@github.com:canonical-cloud/canonical-monorepo.git
```

For an existing checkout:

```sh
git submodule update --init --recursive
```

## Build the full stack

```sh
./build.sh            # builds the Astro frontend, publishes it into the
                      # backend's static/, then builds the Rust backend
```

## Update pins

```sh
scripts/pin-submodules.sh main
git status
git diff --cached --submodule
git commit -m "Pin canonical apps to main"
```

The script verifies the target branch exists on every submodule remote, refuses
dirty submodule checkouts, updates every `.gitmodules` `branch` entry,
fast-forwards each submodule, and stages the resulting gitlink pins. Preview
with `--dry-run`.

## Feature branches

Switch the superproject and every app submodule to the same feature branch:

```sh
scripts/checkout-feature-branch.sh feature/new-landing
```

## Audit

```sh
scripts/audit-repo-state.sh          # conflict markers, stray secrets, submodule wiring
```

## Layout

```text
apps/                  # git submodules (the app repos)
.nix/ .envrc shell     # nix dev shell
scripts/               # pin / checkout / audit helpers (no destructive git)
tests/                 # node --test superproject contract specs
docs/                  # repo-boundaries, deploy
.github/               # CI + dependabot
build.sh               # frontend -> backend static -> cargo build
```
