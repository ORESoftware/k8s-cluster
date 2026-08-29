# Reusable agent-first Nix profiles

`render-nix-agent-profile.sh` renders a reviewed starting contract for seven repository classes:

- `rust` — Rust services and libraries;
- `flutter` — Flutter and Dart clients;
- `node` — Node.js and pnpm applications;
- `go` — Go services and libraries;
- `python` — Python applications and services;
- `kubernetes` — Kubernetes, GitOps, and infrastructure repositories;
- `polyglot` — monorepositories that need a deliberately small shared tool baseline plus repository-owned validation.

The profile definitions live in `nix-agent-profiles/profiles.json`. They are data, not executable repository policy. The renderer turns one profile into a root `flake.nix`, `.nix/profile-packages.nix`, `.nix/dev-shell.nix`, `.nix/agent-check.sh`, `.nix/README.md`, pinned `.github/workflows/nix.yml`, and a narrow managed `.gitignore` block.

## Render a profile

Run from this repository and point `--output-dir` at a clean feature-branch checkout of the target repository:

```sh
nix develop -c bash scripts/render-nix-agent-profile.sh \
  --profile rust \
  --output-dir ../target-repository
```

The renderer refuses to overwrite managed files unless `--force` is supplied. `--force` is intentionally blunt: first review and preserve repository-specific packages, commands, platform conditionals, and comments.

The normal path runs `nix flake lock`. Commit the generated `flake.lock`; a rollout without a committed lock remains incomplete. `--skip-lock` is only for offline fixture tests and prints a warning.

## Repository-specific extension

Generated profiles separate reusable baseline checks from repository-owned behavior:

```sh
nix develop -c agent-check preflight
nix develop -c agent-check profile
nix develop -c agent-check local
nix develop -c agent-check
```

Add native dependencies to `.nix/profile-packages.nix`. Put repository-specific validation in executable `.nix/agent-check.local.sh`; do not edit the central profile merely to accommodate one repository. Kubernetes and polyglot profiles fail closed until this local hook exists because tool-version checks cannot prove that manifests, deployments, or multiple applications are correct.

A local hook must remain non-interactive, deterministic, credential-neutral, and safe to run repeatedly. It must not select cloud profiles, fetch secrets into the working tree, mutate global configuration, deploy resources, publish packages, or write outside the repository-local cache root.

## Classification

Use the fleet classifications consistently:

- `full flake`: root `flake.nix`, committed `flake.lock`, `.nix/`, pinned Nix CI, and a tested non-interactive agent command;
- `shell only`: some Nix support exists, but one or more full-contract elements are missing;
- `not applicable`: there is no default branch or no meaningful repository worktree to develop or validate;
- `deferred with reason`: a concrete blocker or intentional sequencing decision is recorded. “Not audited yet” is not a reason.

Generated output is a starting point, not automatic proof of `full flake`. Run the target repository's native and Nix workflows and record the evidence on its Linear issue.

## Docker and OCI boundary

The profiles do not replace production Dockerfiles or deployment image references. Nix begins as the development and validation layer. Promote a Nix-built OCI output only after service-specific parity is demonstrated for the executable and dynamic closure, non-root UID/GID and file ownership, CA certificates, ports, entrypoint and arguments, startup/shutdown signals, health behavior, size and layers, SBOM and provenance, signatures, vulnerability findings, and deployment compatibility.

Mobile clients and ordinary libraries are not long-running OCI workloads. Polyglot monorepositories retain application-specific image contracts rather than inheriting one generic image.

## Validation

The fixture suite renders every profile into an isolated Git repository, checks generated file structure and permissions, enforces nixfmt/ShellCheck/shfmt, parses the generated workflow, verifies pinned actions and credential neutrality, exercises required and optional local-hook behavior, verifies overwrite protection, and ensures the managed `.gitignore` block is idempotent.

```sh
nix develop -c shellcheck \
  scripts/render-nix-agent-profile.sh \
  scripts/tests/render-nix-agent-profile-test.sh
nix develop -c shfmt -i 2 -ci -d \
  scripts/render-nix-agent-profile.sh \
  scripts/tests/render-nix-agent-profile-test.sh
nix develop -c bash scripts/tests/render-nix-agent-profile-test.sh
```
