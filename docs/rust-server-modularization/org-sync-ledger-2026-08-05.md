# Rust service modularization and organization sync ledger

Updated: 2026-08-05 (America/Lima)

This publication records verified GitHub and Linear state for the HypeSiege and StreemPilot Rust-server modularization program. It deliberately distinguishes merged source, generated starter artifacts, repository provisioning, and organization-project synchronization.

## Executive status

| Organization | Wave 1 runtime modularization | Wave 2 starter contracts | Repository provisioning | GitHub Projects | Linear project |
|---|---|---|---|---|---|
| `hypesiege` | Complete for API, web, and MCP | HypeSiege monorepo PR #1 merged; exact artifact published | Secure GitHub-App provisioning PR #14 is under CI | Projects-v2 mutation is not available through the connected GitHub integration; no board creation is claimed | `github.com/hypesiege` (`cd247cb1-870b-471e-89f6-9484df19e798`) |
| `StreemPilot` | Complete for API, web, and MCP historical service set | Current-main semantic PR #16 contains the media-router starter | Target repository remains `provisioning_required` | Projects-v2 mutation is not available through the connected GitHub integration; no board creation is claimed | `github.com/streempilot` (`3f5bd157-4424-42cc-94d0-0bed993cdc1d`) |
| `ORESoftware` | Central CI/ARC continuity and evidence owner | This publication is the central evidence index | No repository-creation claim is made from this repository | Organization-project credential bridge previously failed closed and was removed; project state must be updated through a Projects-capable credential | `github.com/ORESoftware` (`7abf8be2-ffa5-4507-bd09-43aa59ca8718`) |

## Verified Wave 1 merges

### HypeSiege

| Repository | PR | Reviewed head | Merge commit | Boundary |
|---|---:|---|---|---|
| `hypesiege-mcp-server.rs` | #7 | `f304fbb72e5d735c871e852927133655ab77dd06` | `c5eede9fa269a079581a1a078e807a22400f7ffb` | stdio runtime separated from tools, domain, and telemetry |
| `hypesiege-web-server.rs` | #8 | `ad070f686c7e90202ab7ce4808d8f25ab8731c91` | `d888347290c0fb07048090118b74e7fed7e9e279` | Axum routes, state, security headers, and lifecycle extracted |
| `hypesiege-api-server.rs` | #19 | `86339d2f1ac37cb2c5187af12713c856848a8e10` | `9d3dbf596ad7d0f60b8a3e618f96ebf1702e0bb1` | required Postgres bootstrap and worker ownership extracted |

### StreemPilot

| Historical repository / current alias | PR | Reviewed head | Merge commit | Boundary |
|---|---:|---|---|---|
| `streempilot-api-server.rs` | #19 | `2d23e6adb7b662ca93081a429c8d55c61f480e6a` | `bd6265e7e70f10d01458b7cb1deb7c79bd388c1c` | Postgres/NATS/signaling bootstrap and outbox lifecycle extracted |
| historical `streempilot-web-server.rs`, currently redirected to `sp-web-mash` | #8 | `24663822a66c73bcf972b8e938bcefa550a6691d` | `e937f48e7c4732c998668fe04a0e85455e970f07` | Axum/htmx routes, state, security, validation, and lifecycle extracted |
| historical MCP service repository | prior merged tranche | recorded in DEN-1682 | recorded in DEN-1682 | read-only MCP runtime was separated before the current repository-family rename program |

Repository aliases must be resolved against the current canonical fleet ledger before writing new gitlinks or release automation.

## HypeSiege Wave 2 exact artifact

The merged HypeSiege monorepo contract generates three complete, dependency-free Rust starter repositories:

- `hypesiege/hypesiege-scheduler.rs`
- `hypesiege/hypesiege-publishing-worker.rs`
- `hypesiege/hypesiege-analytics.rs`

Evidence:

- monorepo PR: `hypesiege/hypesiege-monorepo#1`
- exact reviewed head: `2d430aeb9a61b31ecfb71a2d8e57359c24f06f4a`
- merge commit: `aec0f442b1e189f16eec32ede0b4095413405073`
- workflow run: `31032025476`
- workflow artifact ID: `8941056209`
- workflow artifact digest: `sha256:9040cb80f3f875e29b1a0119c26460b86e2c31be3c8bbd326d0c1208759fde69`

The artifact passed independent deterministic renders, offline locked Cargo metadata, rustfmt, strict Clippy, all-target tests, startup probes, archive metadata checks, and SHA-256 provenance checks.

Contained archives:

| Archive | SHA-256 |
|---|---|
| `hypesiege-analytics.rs.tar.gz` | `38e7d2c3545b9bb8c03345b8e994add8b3b07437bfbaf91ad20287b9ef256ee9` |
| `hypesiege-publishing-worker.rs.tar.gz` | `1ced0b767c67b04380b8b444ff32d0f22152cdefe59522ce0c0e4b793f71d05a` |
| `hypesiege-scheduler.rs.tar.gz` | `5a3bf02817e7293d13272e08bf52a219f5fb87f2d99e9c62cf420b552e7b4b35` |

The ZIP is stored beside this ledger at `artifacts/hypesiege-wave2-rust-service-starters.zip`.

## StreemPilot media-router status

Current PR: `StreemPilot/streempilot-monorepo#16`

The change was rebuilt semantically on current `main` after the original branch diverged by fourteen commits. It preserves the newer canonical release ledger while adding:

- the exact `StreemPilot/streempilot-media-router.rs` provisioning manifest;
- a dependency-free modular Rust starter;
- policy-aware credential-marker validation;
- rustfmt-before-provenance rendering;
- deterministic archive tests; and
- offline locked Cargo/Clippy/test/startup validation.

The original GitHub-hosted jobs did not start because GitHub reported failed account payments or an Actions spending limit. The workflow was moved to the existing `[self-hosted, linux, sonus-ci]` ARC label set. The PR remains draft until those exact-head jobs execute successfully. No green or merge claim is made here.

## Repository-creation boundary

The connected GitHub application used for this publication does not expose a direct create-repository operation. Repository creation is therefore implemented only through reviewed, bounded GitHub Actions workflows that:

1. inspect trusted Actions secrets without accepting PAT-shaped fields;
2. validate exactly one GitHub App credential pair against the target organization;
3. require an all-repositories installation and explicit repository-administration permissions;
4. mint and revoke short-lived installation tokens;
5. create only the manifest-approved private repositories; and
6. upload non-secret evidence while shredding temporary credential files.

HypeSiege provisioning is proposed in monorepo PR #14. Repository creation is not considered complete until its post-merge provisioning job returns exact per-repository evidence.

## GitHub Projects and Linear synchronization

### Linear

Canonical projects:

- HypeSiege: `github.com/hypesiege` — project ID `cd247cb1-870b-471e-89f6-9484df19e798`
- StreemPilot: `github.com/streempilot` — project ID `3f5bd157-4424-42cc-94d0-0bed993cdc1d`
- ORESoftware: `github.com/ORESoftware` — project ID `7abf8be2-ffa5-4507-bd09-43aa59ca8718`

Program issues:

- `DEN-1682` — six-service modularization contract and merge evidence
- `DEN-1757` — Wave 2 starter and repository-provisioning program

### GitHub Projects

The currently connected GitHub integration supports repository, branch, commit, issue, pull request, Actions, and artifact operations, but it does not expose Projects-v2 queries or mutations. A prior organization-project credential bridge failed closed and was removed from `ORESoftware/k8s-cluster`; no plaintext credential or successful project mutation resulted.

Consequently:

- this ledger does not claim that organization Project #1 exists or was updated;
- per-organization GitHub issues are used as project-ready tracking items; and
- the issue URLs and Linear documents should be added to organization boards when a Projects-capable GitHub App or GraphQL credential is connected.

## Merge and rollback policy

- Merge only the exact head that produced the recorded checks.
- Do not interpret zero-step GitHub Actions failures as code failures; preserve the billing annotation as evidence and route to ARC where available.
- Resolve divergent branches by rebuilding on current main and combining semantic contracts rather than choosing a side wholesale.
- Generated repository archives remain `provisioning_required` until a real repository exists, its initialization PR passes, and the exact commit is added to the monorepo.
- Roll back with normal revert PRs; do not rewrite shared history.
