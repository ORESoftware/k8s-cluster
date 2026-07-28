# Agent-first Nix contract

The canonical development entrypoint is the repository root:

```sh
nix develop
nix develop -c agent-check
nix run .#agent-check
```

`agent-check` is non-interactive and has named stages for precise CI and agent diagnostics:

```sh
nix develop -c agent-check preflight
nix develop -c agent-check fmt
nix develop -c agent-check check
nix develop -c agent-check test
nix develop -c agent-check audit
```

Cargo, target, Rustup, and XDG state is isolated below `.cache/nix-agent` unless the caller explicitly supplies `CARGO_HOME`, `CARGO_TARGET_DIR`, `RUSTUP_HOME`, `XDG_CACHE_HOME`, or `NIX_AGENT_CACHE_ROOT`. The RustSec scanner comes directly from the committed `flake.lock` package set rather than being compiled during validation. The shell does not select cloud profiles, load secrets, mutate global Git configuration, or require prompts.

The repository now contains a reviewed production Dockerfile and container flags contract. Nix remains the reproducible development and validation layer and does not replace that runtime image. A future Nix-built OCI candidate must separately prove release-binary and dynamic closure parity, non-root UID/GID, CA certificates, ports, entrypoint, health and signal behavior, image size/layers, SBOM and provenance, signing, vulnerability results, and deployment compatibility before becoming authoritative.
