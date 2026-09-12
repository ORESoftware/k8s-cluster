# Nightly portfolio dependency graph and minor updater

This runbook defines the single portfolio-wide dependency controller for the canonical GitHub organizations in `ops/registries/portfolio-project-links.csv`. It runs every night at **02:00 America/Chicago**, follows daylight-saving changes through the workflow's IANA timezone, and keeps scheduling centralized rather than copying cron workflows into every organization.

The controller scans every non-archived, non-fork repository in the registry and emits a machine-readable graph, Graphviz DOT, and a Markdown decision report. It then proposes only dependency changes that satisfy the policy below.

## Non-negotiable update policy

| Available change | Nightly action |
|---|---|
| Patch only, such as `1.2.3 → 1.2.7` | Ignore. Do not open a PR or ticket. |
| Minor, such as `1.2.3 → 1.3.x` | Test and open a PR at the newest passing minor line. |
| Major, such as `1.2.3 → 2.x` | Never mutate. Always create or reuse a Linear issue in the mapped project. |
| Fast-forward movement of `main`, `master`, `release`, `release/*`, or `releases/*` | Treat the branch-tip advancement as a minor lane, then test it exactly like a semver minor. |
| Diverged branch, unrecognized branch, missing current commit, or more candidates than the safety cap | Do not guess. Create or reuse a Linear blocker. |

For a newer minor line, the bot may select the highest patch release inside that newer minor. For example, when the current release is `1.2.3`, `1.3.8` is an eligible **minor** update, while `1.2.8` by itself remains an ignored patch-only update.

## Dependency sources

### Git submodules

`.gitmodules` supplies the URL, path, and optional branch. The current gitlink commit comes from the repository index. An explicit allowed branch is followed; otherwise the target repository's default branch is considered only when it is an allowed `main`/`master`/`release*` lane. Updates use a mode-`160000` gitlink change and never copy dependency source into the parent repository.

### Zed packages

`.zpkg.toml` dependencies are added to the graph. Scalar semver constraints such as `^0.2.4` retain their operator while the numeric version advances. When `.zpkg.lock` exists, the scheduled runner builds the `zed` CLI from an immutable `zed-pkg/zed-cli` commit and regenerates the lock. The change is rejected if Zed modifies any tracked path other than `.zpkg.toml` and `.zpkg.lock`.

Git/branch-shaped Zed dependencies are eligible when the manifest has one explicit `rev`, `sha`, or `commit` field and tracks an allowed branch. The controller preserves either an inline table or a dedicated `[dependencies.<name>]` table while replacing only that pin, then regenerates `.zpkg.lock`. When the manifest tracks a branch but omits the current commit, the controller may recover the exact pin from a matching lock entry. Ambiguous or multiline shapes are sent to the bounded repair endpoint or become Linear work rather than being reserialized heuristically.

Lock-only Zed entries that are not direct `.zpkg.toml` dependencies remain graph evidence. They are advanced through their owning direct dependency and do not generate one Linear ticket per transitive lock row.

### Nix

`flake.lock` GitHub and Git inputs become graph edges. Only inputs named directly by the root flake are mutated: branch-tip candidates are pinned by running `nix flake lock --override-input` for the exact candidate commit, and the resulting lock is accepted only when the root input resolves to that exact revision. Transitive lock nodes remain visible in JSON, DOT, and Markdown evidence but are advanced through their owning direct input instead of producing invalid `--override-input` operations. `flake.nix` is recorded as a manifest surface even though the resolved graph comes from `flake.lock`.

Legacy `nix/sources.json` and lock shapes that require an unavailable ecosystem-specific writer are discovered but are not hand-edited. They go through the repair-or-Linear path.

## Candidate search and testing

Candidates are ordered from oldest to newest. The controller first uses binary search under the usual compatibility assumption that passing versions form a prefix. Because real dependency histories can be non-monotonic, it then verifies untested newer candidates from newest downward. This preserves the requested bisection behavior without missing a later release that fixes an intermediate regression.

Every trial uses a bot-owned branch under `bot/portfolio-deps/`. The branch is reset to the current default-branch head before each candidate, the exact dependency pin is committed and force-updated, and testing is delegated to a fixed `gha-indie-worker` profile. The controller never sends repository-selected shell text to the worker.

Profiles are selected conservatively:

- `nix-verify` for flakes;
- `rust-verify` for root Rust crates;
- `flutter-verify` for Flutter repositories;
- `node-verify` for repositories with a supported Node lockfile;
- `python-verify` for Python repositories.

A repository may select another preinstalled fixed profile through `.portfolio-dependency-bot.json`:

```json
{
  "profile": "rust-verify"
}
```

The named profile must already exist and be allowlisted by the worker. This is not an arbitrary command escape hatch.

## Repair and Linear escalation

When the fixed profile fails, the controller may send a bounded `portfolio-dependency-repair.v1` request to `DEPENDENCY_REPAIR_ENDPOINT`. The payload includes the repository, bot branch, exact dependency and candidate, worker result, and explicit minor-only constraints. At most two repair attempts are allowed, and each claimed repair is retested through the same worker profile.

A Linear issue is created or reused when:

- any major release is observed;
- no minor/branch-tip candidate passes;
- the dependency cannot be rewritten safely;
- the tracked branch diverged or exceeded the candidate cap;
- the repository has no fixed worker profile;
- the per-run PR budget is exhausted after a candidate passes;
- the controller hits another blocker that should not be silently discarded.

The issue is placed in the Linear project mapped to the source repository's GitHub organization. A deterministic marker prevents nightly duplicate issues while an earlier one remains open.

## Pull requests and supersession

A passing update creates or refreshes one pull request per dependency edge. Its body records the old and new pins, source format, target, test profile, worker job, candidate evidence, and the policy classification.

The controller may close an older PR only when all of these are true:

1. its head branch begins with `bot/portfolio-deps/`;
2. its body contains the exact `portfolio-dependency-bot:v1` marker for the same dependency edge;
3. another managed PR supersedes it.

It never closes a human branch, a human-authored dependency PR, or a bot PR for another edge.

## Schedule and concurrency

The workflow is `.github/workflows/nightly-portfolio-dependencies.yml`:

```yaml
on:
  schedule:
    - cron: '0 2 * * *'
      timezone: America/Chicago
```

The controller scans the full canonical registry with three organizations in flight. Repositories inside an organization are processed sequentially to reduce API, clone, and worker bursts. A single workflow concurrency key prevents overlapping nightly runs. Up to 200 passing dependency PRs may be created or refreshed in one run; additional passing updates become bounded follow-up work instead of allowing an unbounded PR storm.

## Protected environment and credentials

Create the protected GitHub environment `portfolio-dependency-bot` and provision:

| Secret | Purpose |
|---|---|
| `PORTFOLIO_DEPENDENCY_GITHUB_TOKEN` | GitHub App installation token or equivalent with repository metadata read, contents read/write, and pull-request read/write across every canonical organization. |
| `LINEAR_API_KEY` | Read mapped projects and create dependency issues. |
| `GHA_INDIE_WORKER_URL` | Authenticated worker base URL. |
| `GHA_INDIE_WORKER_AUTH` | Shared secret sent only in `x-server-auth`. |
| `DEPENDENCY_REPAIR_ENDPOINT` | Optional bounded repair service. Empty disables automated repair. |
| `DEPENDENCY_REPAIR_TOKEN` | Optional bearer token for the repair service. |

Do not use a personal token embedded in a clone URL. Git authentication is passed through an ephemeral `http.https://github.com/.extraheader` environment entry and is never printed by the controller.

The companion worker-enablement change adds `nix-verify`, enables the existing fixed verification profiles, and explicitly allowlists the canonical portfolio organizations. Repository URLs sent by the controller are normalized to lowercase HTTPS identities so the worker's byte-exact prefix policy is deterministic. Exact per-repository profile restrictions still override broad organization prefixes.

## Operation

Credential-free validation, policy checks, unit tests, and an empty-shape report run on every relevant pull request and push.

A manual dry validation is the default:

```bash
python scripts/ops/portfolio_dependency_bot.py --validate-only
```

A protected manual apply can be restricted to one organization or repository through the workflow-dispatch inputs. The full nightly schedule uses the entire canonical registry.

Every apply run uploads:

- `portfolio-dependencies.json` — nodes, typed edges, candidate outcomes, PRs, tickets, and errors;
- `portfolio-dependencies.dot` — Graphviz dependency graph;
- `portfolio-dependencies.md` — human-readable run summary.

## Activation checklist

1. Merge the controller and worker-profile PRs.
2. Create the protected `portfolio-dependency-bot` environment.
3. Provision the cross-organization GitHub App installation token and Linear token through secrets.
4. Deploy the reviewed worker revision containing `nix-verify` and the canonical organization prefix allowlist.
5. Confirm exact per-repository profile rules still enforce any hardened exceptions before broad rollout.
6. Run a manual apply for one test repository, then one test organization.
7. Verify the graph artifacts, one passing PR, one forced-failure Linear issue, a major-version Linear issue, and marker-gated stale-PR closure.
8. Enable the full nightly path by leaving the merged schedule on the default branch.

Until the protected secrets and worker allowlist exist, validation is active but the scheduled mutation job will fail closed rather than opening untested pull requests.
