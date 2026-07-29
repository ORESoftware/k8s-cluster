# scripts

Operator tooling for coordinating the superproject and its app submodules. All
scripts are `bash`, start with `set -euo pipefail`, never `git push`, and offer
`--dry-run`/`--allow-dirty` previews (enforced by `tests/monorepo-contract.test.mjs`).

- `pin-submodules.sh <branch>` — pins every submodule to a branch: verifies the
  branch exists on each remote, fast-forwards each submodule, and stages the
  resulting gitlink pins (the deployable state). The script itself now rejects
  every branch except `main` while the current main-only policy is active.
- `checkout-feature-branch.sh <branch>` — retained for a future branch-policy
  change, but currently unauthorized: agents and operators must keep the
  superproject and every application checkout on `main` and must not create
  feature branches or linked worktrees.
- `audit-repo-state.sh` — pre-deploy safety audit: flags dirty trees, conflict
  markers, tracked/committed secrets, mutable or fail-open workflow inputs,
  unlocked Cargo commands, dependency lifecycle hooks, moving sibling refs,
  non-digest base images, missing Docker Dependabot coverage, unsafe runtime
  identities, unreproducible README commands, readme app-list drift, and
  visibility-policy drift. It also requires an exact `README.md` in every
  non-ignored directory containing tracked files (generated build output,
  vendored gitlinks, `target`, `dist`, and `node_modules` are excluded). Rust
  services are distroless/nonroot by default;
  OS-tool runners require the explicit `tool-runner-nonroot` profile and uid/gid
  `65532:65532`.
- `check-interface-consumers.sh <public|full|--dry-run>` — compiles the
  generated-interface contract checks across the exact pinned app gitlinks.
  Normal CI uses `public`; protected fleet audit and production promotion use
  `full`, which also verifies the private application consumers with their
  explicitly pinned Rust toolchains.
- `check-formal-methods-manifests.py [--self-test]` — validates every public
  Fiducia `formal/fm.toml` schema-v1 contract at the exact reviewed gitlink.
  It normalizes single- and multi-model manifests, checks source and implemented
  adapter paths, requires exact toolchains and bounded execution settings, and
  exercises fail-closed negative cases with `--self-test`.
