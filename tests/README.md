# tests

Contract tests that guard how the superproject wires its app submodules
together. Pure `node --test` (no build step); run with `node --test tests/*.test.mjs`.
CI checks out submodules recursively before running them.

- `monorepo-contract.test.mjs` — asserts `.gitmodules` stays complete and pinned
  to `main`, that `readme.md` and `docs/repo-boundaries.md` classify every app,
  that `.env.example` exposes the required knobs as placeholder-only values, and
  that the `scripts/` ops tooling keeps destructive actions manual with
  dry-run/audit guardrails.
- `integration-sync-contract.test.mjs` — cross-submodule check that the
  local-first sync `ChangeEvent` contract agrees across
  `apps/fiducia-interfaces` (SQL), `apps/fiducia-sync` (Rust core), and its
  TS/JS transport decoders. Skips with an actionable message when the app
  submodules are not checked out.
- `formal-methods-manifest-contract.test.mjs` — validates the four public
  `formal/fm.toml` adopters at their reviewed gitlinks and runs fail-closed
  schema/path/status self-tests for the future `fmctl` contract.
