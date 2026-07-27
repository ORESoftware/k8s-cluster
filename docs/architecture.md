# Repository dependency order

ClipTown changes should land in this order when they span repositories:

1. `cliptown-interfaces`
2. `cliptown-clients` and `cliptown-rust-backend.rs`
3. `cliptown-cli`, `cliptown-flutter`, `cliptown-extension`, and `cliptown-infra`
4. `cliptown-monorepo` submodule pointers
5. `ORESoftware/k8s-cluster` deployment pointer

A parent repository must not point at a feature-branch-only commit. Submodule updates should reference commits reachable from the child repository's merged `main` branch.
