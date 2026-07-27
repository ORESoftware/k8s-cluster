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

Cargo, target, and XDG state is isolated below `.cache/nix-agent` unless the caller explicitly supplies `CARGO_HOME`, `CARGO_TARGET_DIR`, `XDG_CACHE_HOME`, or `NIX_AGENT_CACHE_ROOT`. The shell does not select cloud profiles, load secrets, mutate global Git configuration, or require prompts.

The repository is currently a Rust service source repository and has no production Dockerfile. Nix is the reproducible development and validation layer, not an implicit authorization to invent a runtime image. A future OCI image must separately define and validate the release binary and dynamic closure, non-root UID/GID, CA certificates, ports, entrypoint, health and signal behavior, image size/layers, SBOM and provenance, signing, vulnerability results, and deployment compatibility.
