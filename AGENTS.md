# Repository guidance

- This is an inventory monorepo. Do not edit application files through a gitlink and commit only the outer repository.
- Commit and publish application changes in the application repository, then update the gitlink here.
- Keep all nested submodules initialized and pinned; do not track a floating working tree.
- Use the dot-prefixed `.nix/` directory and verify changes from `nix develop`.
- Keep `.cli-flags.toml` and `tools/flags-2-env` aligned across the repository family.
- Preserve W3C trace context and OpenTelemetry configuration across service boundaries.
- Validate infrastructure, but never run `kubectl apply`, `tofu apply`, `wrangler deploy`, or an Argo CD sync without explicit approval.
- Run `scripts/check` before publication.
