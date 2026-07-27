# Monorepo validation

The monorepo owns repository topology and cross-project documentation. Its CI therefore validates:

- monorepo-owned Markdown;
- the expected set of ClipTown submodules;
- `main` as every submodule's tracked branch;
- reachability of each recorded gitlink;
- the end-device encryption wording shared across the organization.

Language-specific builds and platform tests remain in the standalone child repositories.
