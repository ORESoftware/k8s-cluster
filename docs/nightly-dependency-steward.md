# Nightly dependency steward

The dependency steward is the portfolio-wide, minor-only upgrade controller for
the canonical GitHub organizations listed in
`ops/registries/portfolio-project-links.csv`. The same registry supplies the
Linear project ID used for each organization’s escalation issues.

## Schedule

The workflow runs every night at **02:00 America/Chicago**. GitHub schedules are
UTC-only, so the workflow registers both `07:00 UTC` and `08:00 UTC`; the
controller uses the current `America/Chicago` offset and turns the redundant
lane into an explicit no-op. This preserves 02:00 Central through daylight and
standard time.

The workflow is also manually dispatchable. Manual runs analyze by default;
`apply=true` enables the separate publication phase. Optional organization,
repository, worker, and PR caps support staged rollouts.

## Upgrade policy

Policy order is strict:

1. **Patch-only releases are ignored.** A dependency at `1.4.2` is not moved to
   `1.4.3` by this job.
2. **Minor releases are eligible.** For every newer minor line in the current
   major, the controller considers the highest stable patch published in that
   line. For example, `1.4.2` may test `1.5.7`, `1.6.3`, and `1.7.1`.
3. **Major releases are never changed automatically.** Any observed newer major
   release always produces or reuses a deduplicated issue in the organization’s
   mapped Linear project, independently of baseline test health.
4. Prereleases and tags that are not exact stable `vMAJOR.MINOR.PATCH` or
   `MAJOR.MINOR.PATCH` tags are excluded.
5. Only dependencies owned by an organization in the canonical registry are
   mutated by default. External dependencies still appear in the graph. The
   deliberately exceptional `--allow-external` mode must be selected explicitly.

A `0.x` dependency follows the same requested rule: a newer `0.MINOR` is an
eligible minor; `1.0.0` is a major and therefore a Linear-only migration item.

## Dependency graph

Each run walks eligible default branches at captured immutable commit SHAs and
emits:

- `dependency-graph.json`, for machines and later reconciliation;
- `dependency-graph.dot`, for Graphviz and visual inspection;
- `report.json` and `report.md`, for run outcomes;
- `publish-plan.json` plus digest-addressed patches, for the isolated publication
  phase.

The scanner recognizes:

- `.gitmodules` plus the checked-in gitlink SHA;
- `flake.lock` GitHub inputs;
- GitHub references in other `*.nix` manifests;
- `.zpkg.toml` dependency entries;
- `.zpkg.lock` entries as lock/provenance graph evidence.

`.zpkg.lock` and Nix content hashes are never fabricated or hand-edited. Zed
locks are refreshed through the pinned Zed resolver. Flake inputs are refreshed
through `nix flake lock --override-input`. Generic Nix expressions remain
read-only graph edges unless the repository supplies an explicit, owned
`lock_commands` contract.

## Candidate search and testing

For each mutable edge, the controller first proves the repository’s exact
baseline. Missing tests or a failing baseline are Linear issues, never a pass.
It then uses binary search over ordered minor lines to locate the compatibility
frontier. Because real dependency compatibility can be non-monotonic, every
unproven newer suffix is checked from newest to oldest before the final target
is selected. The result is the newest observed passing minor line, not merely
the first passing midpoint.

A candidate is published only when all configured preparation and test commands
pass after regenerating the appropriate dependency lock. The selected patch is
reapplied and the same commands are run once more to prove reproducibility.

When the newest minor fails, the controller may run a bounded repository-owned
remediation command or call the configured remediation endpoint. A remediation
patch is limited to 2 MB, cannot escape the repository, and cannot modify
workflow files, environment-secret paths, encrypted environment material, or
`.git`. The candidate is accepted only after the full test contract passes with
the compatibility patch. If an older minor passes but the newest does not, the
older verified minor gets a PR and the newer blocked minor gets a Linear issue.
If no minor passes, only the Linear issue is produced.

## Repository contract

Automatic project detection covers root Zed, Nix, Cargo, pnpm, Yarn, npm, Go,
Python, Flutter/Dart, Gradle, and Maven projects. Repositories with composite,
monorepo, private-registry, or unusual test requirements should commit
`.dependency-steward.toml`:

```toml
[dependency_steward]
prepare_commands = [
  "./scripts/bootstrap-ci.sh",
]
test_commands = [
  "cargo test --workspace --locked --all-targets",
  "corepack pnpm --dir web install --frozen-lockfile",
  "corepack pnpm --dir web test",
]
lock_commands = [
  "./scripts/update-nix-or-zed-lock.sh",
]
remediate_command = "./scripts/dependency-compatibility-agent.sh"
timeout_seconds = 7200
excluded_dependencies = [
  "nix-expression:legacy.nix:old-owner/old-package",
]
```

Commands execute from the cloned exact repository SHA. Provider credentials,
GitHub runner tokens, Linear keys, remediation credentials, passwords, API keys,
and authorization variables are stripped from every repository-controlled child
process.

## Publication and stale-PR handling

Analysis and publication are separate jobs:

1. The analysis job receives only the portfolio read credential, runs repository
   code, builds the graph, proves candidates, and writes a digest-addressed plan.
2. The publication job receives write credentials, but never executes repository
   build or test code. It validates the plan contract and patch digest, requires
   the default branch to remain at the exact tested SHA, applies the patch, and
   creates or updates the managed branch and PR. Branch movement fails closed and
   creates a retry issue rather than replaying a patch onto an untested head.

PRs include an opaque `dependency-steward:v1` ownership marker, the exact base
SHA, selected target tag and SHA, test commands, probe results, and whether
compatibility remediation was used. They are intentionally not auto-merged;
normal repository checks and branch protection remain authoritative.

The controller updates an existing PR for the same deterministic branch. After a
new verified PR exists, it closes only older open PRs that carry the same
steward ownership marker and dependency key and whose target is no newer than
the replacement. Human-authored PRs, Dependabot PRs, and unrelated managed PRs
are never swept. The default safety ceiling is 200 PRs per run; verified work
beyond the ceiling is deferred through Linear and retried later.

## Required environment configuration

The workflow uses the existing `portfolio-project-sync` environment and expects:

- `DEPENDENCY_STEWARD_READ_TOKEN`: preferred cross-organization, contents-read
  credential. `PROJECT_SYNC_GITHUB_TOKEN` is a compatibility fallback.
- `DEPENDENCY_STEWARD_GITHUB_TOKEN`: cross-organization contents and pull-request
  write credential. `PROJECT_SYNC_GITHUB_TOKEN` is a compatibility fallback.
- `LINEAR_API_KEY`: issue read/write access for the mapped Linear projects.
- Optional `DEPENDENCY_STEWARD_REMEDIATION_ENDPOINT` and
  `DEPENDENCY_STEWARD_REMEDIATION_TOKEN`.
- Optional `DEPENDENCY_STEWARD_REMEDIATION_COMMAND` environment variable for a
  trusted global compatibility agent; a repository-local command takes
  precedence.
- Optional `DEPENDENCY_STEWARD_LINEAR_TEAM_ID` variable when a mapped project
  cannot supply an unambiguous default team.

The nightly jobs target the isolated `sonus-ci` self-hosted runner label and pin
the controller checkout, Python, Nix, Rust, Node, pnpm, Zed resolver, artifact,
and workflow-validation dependencies. The same-run analysis artifact is retained
for 30 days along with publication evidence.

## Failure behavior

The steward fails closed:

- no test contract means no PR;
- a failing baseline means no candidate testing for that repository;
- unresolved current versions become Linear issues because patch/minor/major
  classification would otherwise be unsafe;
- a moved default branch is never force-rebased;
- a lock-refresh failure is a failed candidate;
- provider failures remain visible in artifacts and the workflow result;
- no PR is auto-merged and no CI result is bypassed.
