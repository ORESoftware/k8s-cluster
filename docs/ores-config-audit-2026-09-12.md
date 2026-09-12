# ORES config and repository audit — 2026-09-12

Tracking: [DEN-2786](https://linear.app/denman/issue/DEN-2786),
[DEN-390](https://linear.app/denman/issue/DEN-390),
[ores-cli#265](https://github.com/ORESoftware/ores-cli/issues/265),
[k8s-cluster#1542](https://github.com/ORESoftware/k8s-cluster/issues/1542).

## Evidence and limits

The baseline was `k8s-cluster@dev` commit
`a2ace8a0bcfd3c777dbcb190dba31fc9116fe453`.
The read-only tool was current `ores-cli@ee73d134332c59b6883e68a36288f1bc8f5af93d`,
with the config diagnostic and boolean-policy fixes subsequently tested at
`c09fc2a` on `salvage/pr-156-DEN-390-k8s-config-audit`.

This is a configuration/topology audit, not a deployment certificate.
No AWS, Hetzner, database, DNS, Kubernetes mutation, repository creation,
visibility change, or credential rotation was performed in this follow-up.
Existing user work in the original checkout was left untouched.

The root audit exited 2. It hit 256-workflow and 500-source scan limits, so its
coverage is incomplete. It also reported a missing root LICENSE, unpinned workflow
actions, generated-directory documentation gaps, and systems-Python review gaps.
Do not resolve the licensing finding by inventing a license grant.

## Changes and executable checks

- Corrected three renamed upstream URLs in `.gitmodules` and `SUBMODULES.md`.
  Local paths and gitlinks are unchanged.
- Moved the browser MCP worker and OAuth secret declarations from public flags
  to `[env].ignore`. The process's existing environment reads, startup checks,
  Kubernetes Secret references, and authentication defaults are unchanged.
  This does not claim the service's broader flags-2-env runtime migration is complete.
- Added this repo's root `zed-env.toml` with explicit local-only test tasks.
- The real ORES CLI accepts the updated browser TOML and reports
  `cliSecretClassPublicFlagCount = 0` (previously 3).
- The browser MCP exposure/config suite passes 6/6, including Secret delivery,
  non-root container, OAuth gateway limits, both cluster registrations, and
  environment-only credential declarations.
- Zed 0.3.0 successfully lists, plans, and executes `zed task run check`.

Run local checks from the repository root:

```sh
zed task run deps
zed task run check
zed task run audit
```

The audit task requires an installed, package-contract-bound `oresc`.
Audit findings deliberately remain nonzero exits. The local `check` task is a
small config regression suite; it does not replace full cluster acceptance.

## Upstream identity corrections

| Existing mount | Canonical repository | Verified unchanged gitlink |
| --- | --- | --- |
| `remote/modules/github/oresoftware/go-iterators` | [ORESoftware/iterators.go](https://github.com/ORESoftware/iterators.go) | `1c545412bd765b209703b3b6dddc9bb9620e1d34` |
| `remote/submodules/sonus-auris-site.web` | [sonus-auris/sonus-auris.github.io](https://github.com/sonus-auris/sonus-auris.github.io) | `299777a00968c32a6f5a3d9ea700dee59983d3ee` |
| `remote/deployments/billing-server-rs` | [quaestor-ledger/quaestor-ledger-server.rs](https://github.com/quaestor-ledger/quaestor-ledger-server.rs) | `a5ffa3e6a8ff312c549f8190950a703a9b2c74a2` |

GitHub's repository endpoint resolves each old name to the canonical name above.
The canonical commit endpoint also returned each exact gitlink. These findings
do not justify creating replacement repositories or upgrading pins without tests.

## Organization-level TOML coverage

The CLI inventoried all 47 referenced upstream names across 19 owners with a
1,000-repository bound per owner; no owner inventory reached that bound.
Three missing-name findings were the redirects above. Runtime code inside the
uninitialized upstream submodules was not audited by this inventory.

| Owner | Referenced upstreams | `.github/governance/source-of-truth.toml` |
| --- | ---: | --- |
| oresoftware | 17 | loaded revision 1 |
| sonus-auris | 4 | loaded revision 1 |
| akrion-sim | 3 | unavailable |
| 3fa-app | 2 | unavailable |
| benefactor-cc | 1 | unavailable |
| daedalus-fab | 2 | unavailable |
| canonical-cloud | 1 | unavailable |
| fiducia-cloud | 3 | unavailable |
| quaestor-ledger | 1 | unavailable |
| sagitta-stack | 1 | unavailable |
| claritas-viz | 1 | unavailable |
| scintilla-run | 2 | unavailable |
| athlet-o | 2 | unavailable |
| usa-acc | 1 | unavailable |
| shared-auth | 3 | unavailable |
| drone-mngr | 1 | unavailable |
| zed-pkg | 1 | unavailable |
| cliptown | 1 | unavailable |
| anticaptrad | 1 | unavailable |

The CLI reports public `.github` visibility for these owners. Existing manifests
also report mirror/digest and repository-role gaps. Governance manifests cannot
be replaced with guessed domain, cloud, Linear, or Slack mappings. Verify the
organization's actual authority and mapping evidence before supplying the 17
unavailable files. Visibility changes need explicit administrative review.

## Embedded deployment config inventory

There are 75 tracked top-level deployment scopes with Cargo, Node, Gleam, Dart,
or Go manifests. Eight have a local `.cli-flags.toml`; 67 do not. A ninth contract
exists in the nested presence-server copy under `gleamlang-ws-server`. All nine
existing contracts parsed successfully when audited individually.

Presence is not runtime adoption evidence. An absent contract is a classification
and integration task, not permission to add unused boilerplate: distinguish an
argv-owning executable from a library, UI-only package, environment-only service,
or fixture; then wire and test the official parser at the actual entrypoint.
Submodule-owned source changes must land in their upstream repositories first.

The scraper's `SCRAPER_ALLOW_URL_CREDENTIALS` is a typed boolean policy, not a
secret value. The CLI fix exempts only typed boolean policy names; string-valued
credentials and secret env keys still fail. No scraper security policy was weakened.

Paths below are relative to `remote/deployments/`.

| Scope | Manifest(s) | CLI contract |
| --- | --- | --- |
| `agent-sim-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `agent-worker-broker-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `apostille-services-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `auth-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `bastion-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `benefactor-orchestrator` | `package.json` | absent — classify runtime before adoption |
| `browser-job-runner-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `browser-mcp-rs` | `Cargo.toml` | present |
| `browser-test-server` | `package.json` | absent — classify runtime before adoption |
| `build-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `chaos-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `ci-profile-runner-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `cluster-mcp-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `constraint-scheduler-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `container-pool-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `contract-service-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `dataset-labeling-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `dd-benefactor-marketing-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `dd-compliance-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `dd-document-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `dd-email-sms-contact-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `dd-embeddings-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `dd-escrow-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `dd-git-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `dd-gpu-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `dd-music-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `dd-ocr-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `des-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `des-simulator-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `dev-server` | `package.json` | absent — classify runtime before adoption |
| `durable-worker-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `economics-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `emulator-testing-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `evolution-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `formal-methods-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `formal-methods-service-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `func-approx-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `gha-capacity-broker-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `gha-clone-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `gleam-mcp-server` | `gleam.toml` | present |
| `gleamlang-presence-server` | `gleam.toml` | present |
| `gleamlang-server` | `gleam.toml` | present |
| `gleamlang-ws-loadtest` | `gleam.toml`, `package.json` | present |
| `gleamlang-ws-server` | `gleam.toml` | present |
| `go-wss-server-go` | `go.mod` | absent — classify runtime before adoption |
| `happy-wakey-gateway-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `idle-reaper-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `in-house-mip-solver-node-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `knowledge-graph-builder-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `live-mutex-loadtest-node` | `package.json` | absent — classify runtime before adoption |
| `lock-loadtest-gleam` | `gleam.toml` | present |
| `lock-loadtest-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `mdp-optimizer-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `monte-carlo-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `patent-filing-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `public-data-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `quantum-compute-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `queue-consumer-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `raft-consensus-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `rest-api-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `route-optimizer-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `routing-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `runtime-config-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `rust-vapi-phone-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `rust-wss-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `sat-smt-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `thread-fleet-exporter-go` | `go.mod` | absent — classify runtime before adoption |
| `thread-operator-go` | `go.mod` | absent — classify runtime before adoption |
| `trading-server-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `wal-gateway-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `web-home-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `web-scraper-service` | `package.json` | present |
| `webrtc-media-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `webrtc-signaling-rs` | `Cargo.toml` | absent — classify runtime before adoption |
| `ws-loadtest-rs` | `Cargo.toml` | absent — classify runtime before adoption |

## Tooling and PR blockers

- ORES CLI formatting, Clippy, repository/config audit, and the new regressions
  pass. Its complete Rust test run is 583 passed / 1 failed across 24 targets.
  The remaining test invokes the real package audit and correctly rejects
  missing Zed lock coordinates for `ecma-d/ecmad-clients@^0.2.0` and
  `ores-rate-limit/ores-rl-lib-core@^0.2.0`. This failure predates this work.
- Zed's registry returned HTTP 503. Approved GitHub fallback also failed:
  `ecmad-clients` has no published tag and the rate-limit core has only v0.1.0,
  although both source manifests declare 0.2.0. Do not fabricate a lockfile,
  silently downgrade, or publish untested producer packages.
- The ORES CLI PR queue had 111 open PRs, zero all-green candidates.
  All 18 non-draft heads were behind current main. Sample run
  [34668050090](https://github.com/ORESoftware/ores-cli/actions/runs/34668050090)
  executed zero steps with a billing/spending-limit annotation. See
  [ores-cli#46](https://github.com/ORESoftware/ores-cli/issues/46).
  A zero-step run is not a passing or failing source test.
- Security work from [ores-cli#156](https://github.com/ORESoftware/ores-cli/pull/156)
  was salvaged onto current main rather than discarding the branch.
  Its original fixture passed even before the fix: the syntax error appeared
  after the secret line. Effective negative controls must place malformed syntax
  on the secret-bearing line and inspect serialized SDK and real CLI output.
  The repaired parser derives numeric locations from its already-bounded input;
  it does not reopen the file to sanitize an error after the fact.

## Remaining work

1. Resolve and certify the two producer package publications, generate a real
   Zed lockfile, and rerun exact-head CLI/package/install gates.
2. Review and populate the 17 missing/unavailable governance TOMLs from verified
   organization mappings, retaining unknown/blocked states where evidence is absent.
3. Migrate the 67 undeclared embedded scopes in owner-aware batches with actual
   parser integration, negative flag tests, env precedence tests, secret handling,
   package retention, and container startup checks.
4. Extend bounded repo audits or orchestrate per-scope runs so scan limits are
   explicit and the nine nested CLI contracts cannot be mistaken for root coverage.
5. Continue the namespace migration and local Kubernetes acceptance work under
   DEN-2786; cloud unavailability and a static audit are not live runtime evidence.
