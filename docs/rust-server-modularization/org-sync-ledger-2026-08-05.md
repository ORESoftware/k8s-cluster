# Rust service modularization and organization sync ledger

Updated: 2026-08-05 (America/Lima)

This ledger records verified GitHub and Linear state for the HypeSiege and StreemPilot Rust-service modularization program. It distinguishes merged source, generated artifacts, real repository provisioning, CI infrastructure, and GitHub Projects synchronization. No unverified completion is implied.

## Organization status

| Organization | Wave 1 modularization | Wave 2 source/artifacts | Repository creation | GitHub Projects | Linear project |
|---|---|---|---|---|---|
| `hypesiege` | API, web, and MCP boundaries merged | Scheduler, publishing-worker, and analytics starter contract merged and artifact published | **Blocked:** trusted provisioning run found no GitHub App candidates; Actions exposed only `GITHUB_TOKEN` | Projects-v2 mutation unavailable through the connected integration | `github.com/hypesiege` (`cd247cb1-870b-471e-89f6-9484df19e798`) |
| `StreemPilot` | Historical API/web/MCP set modularized | Media-router starter is in draft PR #16 on current monorepo `main` | `StreemPilot/streempilot-media-router.rs` remains `provisioning_required` | Projects-v2 mutation unavailable through the connected integration | `github.com/streempilot` (`3f5bd157-4424-42cc-94d0-0bed993cdc1d`) |
| `ORESoftware` | Central evidence and CI-continuity owner | Publishes the exact HypeSiege artifact and this ledger | No repository-creation claim | Projects-v2 credential bridge is unavailable; no board mutation is claimed | `github.com/ORESoftware` (`7abf8be2-ffa5-4507-bd09-43aa59ca8718`) |

## Verified Wave 1 merges

### HypeSiege

| Repository | PR | Reviewed head | Merge commit | Boundary |
|---|---:|---|---|---|
| `hypesiege-mcp-server.rs` | #7 | `f304fbb72e5d735c871e852927133655ab77dd06` | `c5eede9fa269a079581a1a078e807a22400f7ffb` | stdio lifecycle separated from domain, tools, and telemetry |
| `hypesiege-web-server.rs` | #8 | `ad070f686c7e90202ab7ce4808d8f25ab8731c91` | `d888347290c0fb07048090118b74e7fed7e9e279` | Axum routes, state, browser security, and runtime extracted |
| `hypesiege-api-server.rs` | #19 | `86339d2f1ac37cb2c5187af12713c856848a8e10` | `9d3dbf596ad7d0f60b8a3e618f96ebf1702e0bb1` | required Postgres bootstrap and worker ownership extracted |

### StreemPilot

| Historical repository/current alias | PR | Reviewed head | Merge commit | Boundary |
|---|---:|---|---|---|
| `streempilot-api-server.rs` | #19 | `2d23e6adb7b662ca93081a429c8d55c61f480e6a` | `bd6265e7e70f10d01458b7cb1deb7c79bd388c1c` | Postgres/NATS/signaling bootstrap and outbox lifecycle extracted |
| historical `streempilot-web-server.rs`, now redirected to `sp-web-mash` | #8 | `24663822a66c73bcf972b8e938bcefa550a6691d` | `e937f48e7c4732c998668fe04a0e85455e970f07` | Axum/htmx routing, state, validation, response security, and runtime extracted |
| historical MCP service repository | earlier merged tranche | recorded in DEN-1682 | recorded in DEN-1682 | read-only MCP runtime separated before current repository-family renames |

Current names must be resolved through the fleet ledger before new submodule or release automation is written.

## HypeSiege Wave 2 artifact

`hypesiege/hypesiege-monorepo#1` merged from exact head `2d430aeb9a61b31ecfb71a2d8e57359c24f06f4a` as merge commit `aec0f442b1e189f16eec32ede0b4095413405073`.

Workflow run `31032025476` passed:

- independent deterministic renders;
- byte-identical trees and archives;
- offline locked Cargo metadata;
- rustfmt;
- strict Clippy;
- all-target tests;
- bounded startup probes; and
- archive metadata and SHA-256 provenance.

Published GitHub Actions artifact:

- artifact ID: `8941056209`
- artifact SHA-256: `9040cb80f3f875e29b1a0119c26460b86e2c31be3c8bbd326d0c1208759fde69`
- durable path in this publication: `artifacts/hypesiege-wave2-rust-service-starters.zip`

Contained starter archives:

| Archive | SHA-256 |
|---|---|
| `hypesiege-analytics.rs.tar.gz` | `38e7d2c3545b9bb8c03345b8e994add8b3b07437bfbaf91ad20287b9ef256ee9` |
| `hypesiege-publishing-worker.rs.tar.gz` | `1ced0b767c67b04380b8b444ff32d0f22152cdefe59522ce0c0e4b793f71d05a` |
| `hypesiege-scheduler.rs.tar.gz` | `5a3bf02817e7293d13272e08bf52a219f5fb87f2d99e9c62cf420b552e7b4b35` |

## HypeSiege repository-provisioning evidence

The secure GitHub-App provisioning implementation was merged through:

- PR #14, merge `6b4adfa919eaf5b7cc8c931f9ac4e04603a72579`;
- PR #16, merge `7460f5109c2ae0711a75821094faa6629925b381`, adding the actor/issue/body-bound trigger; and
- issue #15 exact command `provision-hypesiege-wave2:15:v1`.

Trusted-main workflow run `31036069654` behaved fail-closed:

- contract validation: success;
- repository provisioning: failure before mutation;
- observed Actions secret context: only the ephemeral `github_token` field;
- GitHub App candidates: `app_ids=0`, `private_keys=0`;
- repositories created: **0**;
- PAT fallback: **not used**.

Therefore these targets remain generated artifacts, not existing repositories:

- `hypesiege/hypesiege-scheduler.rs`
- `hypesiege/hypesiege-publishing-worker.rs`
- `hypesiege/hypesiege-analytics.rs`

To unblock, install a repository-admin GitHub App secret pair in a trusted organization/repository secret scope, or connect a repository-creation capability. The workflow must then be rerun; code initialization and monorepo gitlinks remain later PR gates.

## StreemPilot media-router status

Current PR: `StreemPilot/streempilot-monorepo#16`.

Exact current head: `850a82afa51b3ee3f9581aa077bcc54aa489ff34`.

The change was rebuilt semantically on current monorepo `main` after the original branch diverged by fourteen commits. It preserves the newer canonical fleet/release ledger while adding:

- the exact `StreemPilot/streempilot-media-router.rs` provisioning manifest;
- a dependency-free modular Rust starter;
- bounded deterministic RTMP/SRT route planning;
- policy-aware credential-marker checks;
- rustfmt-before-provenance rendering; and
- deterministic offline validation workflows.

GitHub-hosted jobs failed before checkout with zero steps because GitHub annotated the account with failed recent payments or an Actions spending-limit requirement. Jobs moved to `[self-hosted, linux, sonus-ci]` remain queued because that scale set has not claimed StreemPilot organization work. No green or merge claim is made. The target repository remains `provisioning_required`.

## GitHub Projects and Linear

### Linear documents

- HypeSiege: `5ec99554-f2cf-4c2b-b244-5af38fc4fc78`
- StreemPilot: `066d5d11-cc79-4140-9e36-9f04a9ca56f8`
- ORESoftware: `f0142f3a-d057-4988-af26-0d0ef74eb3ad`

Program issues:

- DEN-1682 — six-service modularization and exact merge evidence;
- DEN-1757 — Wave 2 starters and repository provisioning.

### Project-ready GitHub issues

- HypeSiege: `hypesiege/hypesiege-monorepo#15`
- StreemPilot: `StreemPilot/streempilot-monorepo#17`
- ORESoftware: `ORESoftware/k8s-cluster#988`

### Projects-v2 limitation

The connected GitHub integration exposes repository, branch, commit, issue, pull-request, Actions, and artifact operations, but no Projects-v2 query or mutation. Searches found no installable Projects plugin. No organization board creation, project-number assumption, or item addition is claimed. The issues and Linear documents above are ready to attach once a Projects-capable GitHub App or narrowly scoped GraphQL credential is connected.

## Trust and rollback policy

- No classic PAT was used, stored, committed, or copied into Linear.
- Repository creation remains GitHub-App installation-token only.
- Zero-step billing failures are infrastructure evidence, not source-test failures.
- Generated starters are not marked integrated until real repositories, green initialization PRs, and exact monorepo gitlinks exist.
- Divergent branches are rebuilt semantically on current `main`; one side is never selected wholesale.
- Rollback uses ordinary revert PRs; shared history is not rewritten.
