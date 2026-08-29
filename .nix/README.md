# Nix development contract

This repository keeps the flake entrypoint at `flake.nix` and its implementation under `.nix/`.

## Agent entrypoints

From a fresh clone:

```sh
nix develop
nix develop -c agent-check
nix run .#agent-check
nix flake check --show-trace
```

`agent-check` is deliberately non-interactive. It validates Git whitespace, Nix formatting, shell formatting and linting, the Nix workflow, and all flake checks. It uses repository-local caches below `.cache/nix-agent/` unless the caller supplies explicit cache locations.

The default shell contains the cross-language tools needed to inspect and operate this polyglot Kubernetes repository. Individual deployments can still define narrower nested environments when their requirements differ.

## Credential policy

Entering the shell does not choose an AWS profile, Kubernetes context, cloud account, token, or secret. Agents and operators must select credentials explicitly in the command environment. This prevents `nix develop` from changing identity merely by entering the repository.

## Docker and OCI policy

The Nix shell is a build and validation environment, not a runtime base image. Existing service Dockerfiles and GitOps image contracts remain authoritative until a Nix-built OCI output demonstrates equivalent:

- binary and dynamic-library closure;
- non-root user and filesystem permissions;
- CA certificates, ports, entrypoint, and health behavior;
- image size and layer composition;
- SBOM, provenance, signature, and vulnerability results.

Do not copy the full development closure into production images. Add service-specific `packages.<system>.oci` outputs only where they can be tested against the existing container contract.
