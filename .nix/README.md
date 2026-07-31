# Agent-first Nix contract

The canonical development entrypoint is the repository root:

```sh
nix develop --no-update-lock-file
nix develop --no-update-lock-file -c agent-check
nix run --no-update-lock-file .#agent-check
```

`agent-check` is non-interactive and has named stages for precise CI and agent diagnostics:

```sh
nix develop --no-update-lock-file -c agent-check preflight
nix develop --no-update-lock-file -c agent-check fmt
nix develop --no-update-lock-file -c agent-check check
nix develop --no-update-lock-file -c agent-check clippy
nix develop --no-update-lock-file -c agent-check test
nix develop --no-update-lock-file -c agent-check audit
```

The exact Rust 1.95.0 toolchain (including Cargo, rustfmt, Clippy, and standard-library sources for rust-analyzer) and the RustSec scanner come from the package set pinned by `flake.lock`; validation never bootstraps a mutable rustup toolchain. The development shell isolates Cargo, target, and XDG state below `.cache/nix-agent` unless the caller relocates the cache or explicitly supplies one of those paths. `agent-check` additionally uses an isolated home directory below that cache.

`agent-check` disables interactive prompts, SSH agent/askpass inputs, and global or system Git configuration for its subprocesses. It still inherits ordinary environment variables from the caller, as does the surrounding `nix develop` shell; no repository hook selects a cloud profile or sources a secret file.

The audit stage fetches the locked crates and registry metadata before denying vulnerabilities and every RustSec warning category, including yanked, unmaintained, and unsound packages. It also proves `rsa` is absent from the lock and `sqlx-mysql` is inactive in the PostgreSQL-only graph; DEN-538 removed the former advisory exception rather than weakening audit policy.

The repository now contains a reviewed production Dockerfile and container flags contract. Nix remains the reproducible development and validation layer and does not replace that runtime image. A future Nix-built OCI candidate must separately prove release-binary and dynamic closure parity, non-root UID/GID, CA certificates, ports, entrypoint, health and signal behavior, image size/layers, SBOM and provenance, signing, vulnerability results, and deployment compatibility before becoming authoritative.
