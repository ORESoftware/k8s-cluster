# .nix

Nix flake defining the reproducible Fiducia development shell.

- `flake.nix` — a cross-platform (`x86_64`/`aarch64`, Linux/Darwin) dev shell
  bundling the Rust toolchain (rustc, cargo, clippy, rustfmt, rust-analyzer,
  bacon), Node/pnpm, and supporting tools (git, direnv, just, pkg-config,
  openssl).
- `flake.lock` — pins the exact `nixpkgs` revision for reproducibility.

The shell is entered automatically via the repo's `.envrc` (`use flake ./.nix`)
under direnv, or on demand through the `./shell` launcher.
