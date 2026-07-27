# ClipTown development toolchain

This document defines the first reproducible, non-production ClipTown workspace contract. It intentionally covers local validation and CI parity only. It does not install signing identities, production secrets, cloud credentials, or cluster access.

## Bootstrap

Install [mise](https://mise.jdx.dev/) through its official package or installer, then run:

```sh
bash scripts/bootstrap.sh
```

The bootstrap command installs the reviewed tools from `mise.toml`, initializes all Git submodules, and runs the non-secret preflight diagnostics.

## Validation commands

Run the low-risk contract, packaging, privacy, and GitOps checks:

```sh
bash scripts/validate-workspace.sh
```

Run the complete local language suites:

```sh
bash scripts/validate-workspace.sh --full
```

The full command does not deploy services, migrate a database, publish packages, upload artifacts, or contact production systems.

## Compatibility matrix

| Tool or surface | Local contract | Matching CI boundary |
| --- | --- | --- |
| Rust | 1.88.0 | Backend minimum and locked dependency graph |
| Node.js | 22.x | Interfaces, clients, extension, and monorepo jobs |
| Dart | 3.12.2 | Dart SDK client formatting and analysis |
| Flutter | Stable channel | Flutter quality, desktop, Android, and iOS jobs |
| Java | Temurin 17 | Android build and emulator jobs |
| Android | API 35 | Hosted Android emulator |
| Python | 3.12.x | Wire-contract and repository utility scripts |
| Helm | 3.17.3 | Strict GitOps lint and render job |
| markdownlint-cli | 0.45.0 | Monorepo-owned Markdown lint |
| Buf | CI action v1 | Protobuf lint and additive breaking-change gate |

Flutter and Buf are still selected through reviewed CI actions rather than exact binary versions. Freezing their binary versions is a follow-up DEN-57 task after the active Flutter foundation PR lands.

## Platform responsibilities

### Linux

Linux is the default environment for contracts, Rust, TypeScript, Dart analysis, extension tests, Helm rendering, and non-device Flutter tests. Rust clipboard and keyring builds also need the X11, Secret Service, and D-Bus development headers documented by the CLI workflow.

### macOS

macOS owns iOS simulator execution, macOS desktop builds, future notarization rehearsal, and Apple signing diagnostics. Signing identities and App Store credentials are not required for ordinary validation.

### Windows

Windows owns native desktop and CLI release-build verification. The shared Cargo lockfile must resolve without updates on Windows as well as Linux and macOS.

## Deferred production-only tooling

The following items remain deliberately outside this low-risk slice:

- PostgreSQL and pgvector schema rehearsal containers
- Kubernetes and Kustomize version pinning
- OpenAPI code-generator pinning
- DPM installation and database migration execution
- Apple, Microsoft, Google Play, or browser-store signing credentials
- production Supabase, R2, AWS, Hetzner, or Kubernetes credentials

Those additions must remain reviewable and must not be prerequisites for contract or unit-test workflows.
