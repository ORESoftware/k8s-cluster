# scripts

Operator tooling for coordinating the superproject and its app submodules. All
scripts are `bash`, start with `set -euo pipefail`, never `git push`, and offer
`--dry-run`/`--allow-dirty` previews (enforced by `tests/monorepo-contract.test.mjs`).

- `pin-submodules.sh <branch>` — pins every submodule to a branch: verifies the
  branch exists on each remote, rewrites `.gitmodules`, fast-forwards each
  submodule, and stages the resulting gitlink pins (the deployable state).
- `checkout-feature-branch.sh <branch>` — switches the superproject and every
  submodule to the same feature branch, creating it from the base when needed;
  refuses dirty checkouts.
- `audit-repo-state.sh` — pre-deploy safety audit: flags dirty trees, conflict
  markers, tracked/committed secrets, missing Dockerfiles, non-distroless Rust
  runtimes, readme app-list drift, and non-private superproject visibility.
