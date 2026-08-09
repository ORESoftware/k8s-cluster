# GitHub organization, Linear, and GitHub Project registry

The machine-readable governance registry is [`ops/portfolio/github-linear-project-registry.tsv`](../../ops/portfolio/github-linear-project-registry.tsv). It contains the canonical GitHub organization login and Linear project URL for the current 71-organization fleet. GitHub organization URLs, public governance repositories, canonical Project titles, Project numbers, and Project URLs are derived deterministically from that source.

## Registry scopes

This repository maintains two related but intentionally different inventories.

| Inventory | Scope | Source of truth |
| --- | --- | --- |
| Organization governance | All 71 managed GitHub organizations, including production, test, and governance-only organizations | [`ops/portfolio/github-linear-project-registry.tsv`](../../ops/portfolio/github-linear-project-registry.tsv) |
| Active cross-system portfolio | The active 41-portfolio subset routed across ChatGPT project names, GitHub Projects v2, Linear, and Slack | [`ops/registries/portfolio-project-links.csv`](../../ops/registries/portfolio-project-links.csv) |

The active 41-portfolio subset is a strict subset of the 71 managed organizations. Every overlapping organization must use the same Linear project URL in both files. `portfolio_key` is the lowercase cross-system join key and must equal `github_org.casefold()`; GitHub's canonical organization casing remains authoritative in `github_org` and the governance TSV.

## Operating contract

- GitHub repositories, pull requests, commits, CI, releases, and deployable artifacts are authoritative in GitHub.
- Product planning, dependencies, ownership, milestones, and status are authoritative in the linked Linear project.
- Slack and ChatGPT project metadata are routing surfaces, not alternative planning databases.
- The GitHub organization login is unique case-insensitively and is preserved with canonical casing.
- The canonical GitHub Project title is `<canonical-org-login>-project`.
- The canonical Project URL is `https://github.com/orgs/<organization>/projects/1`.
- `dancing-dragons` uses Project `4`; every other organization uses Project `1`.
- Every organization must maintain a public `<org>/.github` repository with a profile, community-health defaults, root agent instructions, Copilot instructions, contribution/security/support policy, issue forms, a pull-request template, and the canonical Linear backlink.

### Explicit Linear project sharing

Linear project URLs are unique by default. Linear project sharing is allowed only for these exact production/test ownership pairs:

- `flags-2-env` and `flags-2-env-test` share the canonical `github.com/flags-2-env` Linear project;
- `networking-components` and `networking-components-test` share the canonical `github.com/networking-components` Linear project;
- `ores-otel` and `ores-otel-test` share the canonical `github.com/ores-otel` Linear project.

The validators require the complete expected pair for each shared URL and reject every unlisted duplicate, partial exception, or additional owner. Dedicated test projects remain dedicated wherever the registry gives production and test organizations different Linear URLs.

## Conflict-resolution and preservation policy

Registry and documentation conflicts must be resolved semantically, with both complete sides, the merge base, surrounding code and documentation, tests, schemas, generated artifacts, and relevant public contracts in view. Review 3–10 relevant prior commits when useful and inspect related repositories in this organization and relevant external organizations when behavior crosses repository boundaries.

Never blindly choose `ours`, `theirs`, `current`, or `incoming`, and never resolve conflicts by blindly choosing one side. Preserve compatible intent and historical evidence. Managed blocks may be replaced only inside their markers; all unmanaged text and stronger repository-local policy must remain byte-for-byte intact unless an intentional, reviewed change says otherwise.

Automated agents must not use or recommend destructive or history-rewriting operations to reconcile this registry. The hard denylist includes every form of `git stash`, `git reset`, `git clean`, `git filter-repo`, `git filter-branch`, BFG, history-rewriting rebase or amend, destructive checkout or restore, force push, ref deletion or pruning, recursive deletion, destructive database or infrastructure actions, credential revocation, package or release unpublishing, artifact purges, and policy or required-check bypasses.

Prefer read-only inspection, additive branches, separate clean worktrees or clones, explicit staging, ordinary non-force pushes, dry runs, backups, additive migrations, and reversible roll-forward changes.

## Validation

[`scripts/ci/check-github-linear-project-registry.mjs`](../../scripts/ci/check-github-linear-project-registry.mjs) validates the canonical governance registry and this contract. It verifies:

- exactly 71 organization rows;
- strict two-column LF-only TSV syntax;
- canonical, sorted, case-insensitively unique GitHub organization logins;
- HTTPS `linear.app/denman/project/...` URLs without credentials, queries, fragments, alternate ports, or ambiguous redirects;
- unique Linear ownership except for the three exact production/test pairs above;
- Project `1` for every organization except `dancing-dragons`, which uses Project `4`;
- required documentation, semantic conflict-resolution, preservation, and credential-safety language.

[`scripts/ops/validate_github_linear_registry_relationship.py`](../../scripts/ops/validate_github_linear_registry_relationship.py) validates the relationship between the 71-organization governance registry and the active 41-portfolio subset. The expected relationship is:

- 71 governance organizations;
- 41 active portfolio organizations;
- 41 overlapping organizations with identical Linear URLs;
- 30 governance-only organizations;
- no active portfolio organization absent from the governance registry.

The relationship validator also preserves canonical casing, Project title/number/URL rules, immutable Linear URLs, secret-shape rejection, and the exact explicit Linear-sharing exceptions.

The dedicated workflows run positive and negative fixtures, syntax checks, relationship tests, generated-directory checks, whitespace checks, conflict-marker checks, and credential-shape scans before a registry change can be accepted.

## Human-readable directory

[`docs/portfolio/github-linear-projects-by-org.md`](github-linear-projects-by-org.md) is the generated organization directory. It must be regenerated from the TSV whenever registry rows change. Humans and agents should use it for navigation, but the TSV remains authoritative.

## Evidence boundaries

The August 5, 2026 Project/documentation reconciliation campaign was built around the historical 64-organization registry. Its retained run reports and cancellation/replacement evidence must remain labeled as that 64-organization campaign; they must not be mechanically rewritten to 71.

The current 71-organization public `.github` governance rollout is tracked separately in [`ORESoftware/k8s-cluster#1222`](https://github.com/ORESoftware/k8s-cluster/issues/1222). Trusted-main workflow run [`31284729674`](https://github.com/ORESoftware/k8s-cluster/actions/runs/31284729674) verified all 71 repositories, created the six previously missing public `.github` repositories, and merged 71 ordinary pull requests without force push, history rewrite, destructive checkout, branch deletion, or required-check bypass.

A cancelled or incomplete run is not completion evidence. A retained acceptance report must identify the exact commit, exact organization count, created repositories, merged pull requests, final default-branch verification, zero unverified organizations, and cleanup of ephemeral credential transport. Do not expose credentials in source, logs, comments, workflow inputs or outputs, artifacts, generated documentation, or validation errors.

## Adding or changing an organization

1. Confirm the canonical GitHub organization login and canonical Linear project URL.
2. Decide whether the organization owns a dedicated Linear project or belongs to one of the explicitly reviewed production/test sharing pairs.
3. Insert the TSV row in case-insensitive ASCII organization order.
4. Update the expected organization count, sharing exceptions, relationship tests, workflow assertions, and this documentation atomically.
5. Regenerate [`github-linear-projects-by-org.md`](github-linear-projects-by-org.md).
6. Reconcile the public `.github` repository through an ordinary branch and pull request while preserving unmanaged content and stronger local policy.
7. Run the JavaScript validator, Python relationship validator, focused negative fixtures, secret scans, and generated-directory checks.
8. Link the GitHub pull request and the canonical Linear work item in both directions.
