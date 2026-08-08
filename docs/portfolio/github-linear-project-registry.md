# GitHub organization, Linear, and GitHub Project registry

The machine-readable governance registry is [`ops/portfolio/github-linear-project-registry.tsv`](../../ops/portfolio/github-linear-project-registry.tsv). It contains the canonical GitHub organization login and Linear project URL for the current 64-organization fleet. The GitHub organization URL, governance repository, canonical Project title, Project number, and Project URL are derived deterministically from that source.

## Registry scopes

This repository maintains two related but intentionally different inventories.

| Inventory | Scope | Source of truth |
| --- | --- | --- |
| Organization governance | All 64 managed GitHub organizations, including test and governance-only organizations | [`ops/portfolio/github-linear-project-registry.tsv`](../../ops/portfolio/github-linear-project-registry.tsv) |
| Active cross-system portfolio | The 41 portfolios routed across ChatGPT project names, GitHub Projects v2, Linear, and Slack | [`ops/registries/portfolio-project-links.csv`](../../ops/registries/portfolio-project-links.csv) |

The 41 active portfolios are a strict subset of the 64 managed organizations. Every overlapping organization must use the same Linear project URL in both files. `portfolio_key` is the lowercase cross-system join key and must equal `github_org.casefold()`; GitHub's canonical organization casing remains authoritative in `github_org` and the governance TSV.

## Operating contract

- GitHub repositories, pull requests, commits, CI, releases, and deployable artifacts are authoritative in GitHub.
- Product planning, dependencies, ownership, milestones, and status are authoritative in the linked Linear project.
- Slack and ChatGPT project metadata are routing surfaces, not alternative planning databases.
- Each organization has one canonical active GitHub Project titled `<canonical-org-login>-project`.
- The canonical Project is normally project `1`. `dancing-dragons` retains its pre-existing canonical project `4`.
- Organization-level documentation lives in the public `<org>/.github` repository.
- The organization Project contains a durable governance issue linking GitHub, Linear, and the organization documentation.
- Organization names and Linear project URLs are unique, case-insensitively for GitHub organization ownership.
- Registry rows are sorted by canonical organization login and contain no query strings, fragments, embedded credentials, or credential-shaped values.
- Every cross-system issue, event, agent run, and synchronization record should carry `portfolio_key` plus provider-native IDs. Match by native ID first and canonical key second; never infer identity solely from display text.
- Documentation conflicts are resolved semantically against the latest default branch. Managed routing blocks are regenerated while unrelated prose is preserved; automation must never resolve conflicts by blindly choosing one side.

The richer naming, Slack, ChatGPT, native-ID, and marker contract is documented in [`docs/portfolio-project-linking.md`](../portfolio-project-linking.md).

## Derived link contract

For each governance TSV row with organization `<org>`:

| Resource | Derived value |
| --- | --- |
| Organization | `https://github.com/<org>` |
| Governance repository | `https://github.com/<org>/.github` |
| Project title | `<org>-project` |
| Project URL | `https://github.com/orgs/<org>/projects/1` |
| Linear project | Exact `linear_url` from the registry |

The sole Project-number exception is `dancing-dragons`, whose canonical Project URL ends in `/projects/4`.

## Validation

[`scripts/ci/check-github-linear-project-registry.mjs`](../../scripts/ci/check-github-linear-project-registry.mjs) validates the full 64-organization governance registry without network or credentials. It rejects:

- missing, additional, malformed, or unsorted rows;
- duplicate organization ownership, including case variants;
- duplicate or malformed Linear project URLs;
- invalid GitHub organization logins;
- drift in the Project-number exception;
- credential-bearing or ambiguous URLs;
- missing semantic conflict-resolution documentation.

[`scripts/ops/validate_github_linear_registry_relationship.py`](../../scripts/ops/validate_github_linear_registry_relationship.py) validates the relationship between the 64-org governance registry and the 41-row active portfolio registry. It proves that every active portfolio exists in the governance fleet, uses the exact same Linear project URL, preserves canonical GitHub casing, uses the lowercase `portfolio_key`, and derives the expected Project title, number, and URL. It also reports the governance-only organization set.

The permanent [`github-linear-project-registry.yml`](../../.github/workflows/github-linear-project-registry.yml) workflow validates the standalone 64-org registry. The permanent [`validate-portfolio-project-links.yml`](../../.github/workflows/validate-portfolio-project-links.yml) workflow validates the richer 41-portfolio registry, the cross-registry relationship, positive and negative fixtures, and a credential-free provider reconciliation plan.

Run the local credential-free checks before any provider mutation:

```bash
node scripts/ci/check-github-linear-project-registry.mjs
python3 scripts/ops/validate_github_linear_registry_relationship.py
python3 scripts/ops/validate_portfolio_project_links.py
python3 -m unittest \
  scripts/ops/tests/test_github_linear_registry_relationship.py \
  scripts/ops/tests/test_portfolio_project_links.py \
  scripts/ops/tests/test_validate_org_project_docs_evidence.py
```

## Reconciliation

Run [`scripts/ops/sync_org_project_docs.sh`](../../scripts/ops/sync_org_project_docs.sh) with an authenticated GitHub CLI session that can administer the listed organizations. [`scripts/ops/sync_org_project_docs_rate_aware.py`](../../scripts/ops/sync_org_project_docs_rate_aware.py) performs the same fail-closed reconciliation when GitHub API capacity is constrained. The publisher may create or reopen the canonical Project, initialize the public `.github` repository, update only managed routing blocks, open or reuse a normal pull request, and attach the durable governance issue to the canonical Project.

[`scripts/ops/sync_portfolio_project_links.py`](../../scripts/ops/sync_portfolio_project_links.py) separately reconciles the 41 active ChatGPT/GitHub/Linear/Slack mappings. [`scripts/ops/sync_github_project_metadata.py`](../../scripts/ops/sync_github_project_metadata.py) reconciles GitHub Project readmes and short descriptions from the active portfolio registry.

Fleet mutation and read-only registry validation are intentionally separate. A green registry check proves the declared mapping is internally coherent; it does not by itself claim that all Projects, repositories, issues, or pull requests were successfully reconciled remotely.

## Evidence status

GitHub Actions run `31033274687` is quarantined and is **not** acceptance evidence. After GitHub rate limiting began, REST `403` payloads were rendered as organization identities and incomplete rows were marked successful. The incident is recorded in [`ops/evidence/org-project-docs/INVALID-RUN-31033274687.md`](../../ops/evidence/org-project-docs/INVALID-RUN-31033274687.md), and [`audit.json`](../../ops/evidence/org-project-docs/audit.json) records `is_valid: false`.

Do not use that run's `README.md`, `results.json`, or `results.jsonl` to claim that all 64 Projects, `.github` repositories, documentation pull requests, governance issues, or Project items were reconciled.

A replacement run becomes acceptance evidence only when it:

1. contains exactly 64 unique requested organizations from the governance registry;
2. passes [`scripts/ops/validate_org_project_docs_evidence.py`](../../scripts/ops/validate_org_project_docs_evidence.py);
3. has internally consistent Project, repository, pull-request, governance-issue, and Project-item identifiers and URLs;
4. contains no REST, GraphQL, rate-limit, or other API-error payloads;
5. completes live verification while preserving unrelated organization documentation.

Credentials belong in a protected environment or process environment. Never place a PAT in command-line arguments, registry files, documentation, evidence, workflow inputs, issues, or pull-request bodies.

## Adding or changing an organization

1. Update the 64-organization governance TSV in case-insensitive sorted order.
2. For an active portfolio, update the 41-row CSV with the exact GitHub Project, Linear UUID/URL, and Slack IDs.
3. Run the standalone, cross-registry, and provider-specific validators.
4. Reconcile provider metadata through the reviewed workflow.
5. Commit only validated, redacted evidence. Preserve failed evidence as an explicitly quarantined incident instead of rewriting it to appear successful.
