# GitHub organization, Linear, and GitHub Project registry

This repository maintains two related but intentionally different inventories.

| Inventory | Scope | Source of truth |
|---|---|---|
| Organization governance | All 64 managed GitHub organizations, including test and governance-only organizations | [`ops/portfolio/github-linear-project-registry.tsv`](../../ops/portfolio/github-linear-project-registry.tsv) |
| Active cross-system portfolio | The 41 portfolios routed across ChatGPT project names, GitHub Projects v2, Linear, and Slack | [`ops/registries/portfolio-project-links.csv`](../../ops/registries/portfolio-project-links.csv) |

The 41-portfolio inventory is a strict subset of the 64-organization inventory. Every overlapping organization must use the same Linear project URL in both files. [`scripts/ops/validate_github_linear_registry_relationship.py`](../../scripts/ops/validate_github_linear_registry_relationship.py) enforces that relationship, organization casing, Project numbering and URLs, sorted governance rows, uniqueness, and credential-shape rejection.

## Canonical identities

- `organization` / `github_org` is the GitHub organization login, preserving GitHub's canonical casing.
- `portfolio_key` is the lowercase cross-system join key. It must equal `github_org.casefold()`.
- The canonical GitHub Project title is `<github_org>-project`.
- The canonical GitHub Project is normally Project `1`; `dancing-dragons` retains its reviewed Project `4`.
- The Linear project is identified by its native UUID and direct URL. Display-name aliases must not create another project.
- Organization-wide documentation and governance live in the public `<org>/.github` repository.
- The richer portfolio registry also stores the canonical Slack workspace/channel identity and direct URL.

## Authority and synchronization

- Repositories, commits, pull requests, checks, releases, and deployable artifacts are authoritative in GitHub.
- Planning, ownership, dependencies, milestones, and status are authoritative in the linked Linear project.
- Slack and ChatGPT project metadata are routing surfaces, not alternative planning databases.
- Every cross-system record should carry `portfolio_key` and the provider-native IDs. Match by native ID first, then the canonical key; never infer identity solely from display text.
- Managed documentation blocks may be regenerated, but unrelated human prose must be preserved. Resolve conflicts semantically against the latest default branch; never choose an entire conflict side merely because it is newer.

The detailed cross-system naming and marker contract is documented in [`docs/portfolio-project-linking.md`](../portfolio-project-linking.md). The daily provider reconciliation is documented in [`docs/daily-portfolio-project-sync.md`](../daily-portfolio-project-sync.md).

## Evidence status

The committed output from GitHub Actions run `31033274687` is quarantined and is **not** acceptance evidence. GitHub rate-limit responses were previously rendered as organization identities while incomplete rows were marked successful. The incident and repair boundary are recorded in [`ops/evidence/org-project-docs/INVALID-RUN-31033274687.md`](../../ops/evidence/org-project-docs/INVALID-RUN-31033274687.md), and [`audit.json`](../../ops/evidence/org-project-docs/audit.json) records `is_valid: false`.

Do not use the quarantined `README.md`, `results.json`, or `results.jsonl` to claim that all 64 Projects, `.github` repositories, documentation pull requests, governance issues, or Project items were reconciled. A replacement run becomes acceptance evidence only when it:

1. contains exactly 64 unique requested organizations from the governance registry;
2. passes [`scripts/ops/validate_org_project_docs_evidence.py`](../../scripts/ops/validate_org_project_docs_evidence.py);
3. has internally consistent Project, repository, pull-request, governance-issue, and Project-item URLs and identifiers;
4. contains no REST/GraphQL error or rate-limit payloads;
5. completes the live verification phase without rewriting unrelated documentation.

## Validation and reconciliation

Run the credential-free local checks before any provider mutation:

```bash
python3 scripts/ops/validate_github_linear_registry_relationship.py
python3 scripts/ops/validate_portfolio_project_links.py
python3 -m unittest \
  scripts/ops/tests/test_github_linear_registry_relationship.py \
  scripts/ops/tests/test_portfolio_project_links.py \
  scripts/ops/tests/test_validate_org_project_docs_evidence.py
```

Provider reconciliation is split by responsibility:

- [`scripts/ops/sync_portfolio_project_links.py`](../../scripts/ops/sync_portfolio_project_links.py) reconciles the 41 active ChatGPT/GitHub/Linear/Slack portfolio mappings.
- [`scripts/ops/sync_github_project_metadata.py`](../../scripts/ops/sync_github_project_metadata.py) reconciles GitHub Project readmes and short descriptions.
- [`scripts/ops/sync_org_project_docs_rate_aware.py`](../../scripts/ops/sync_org_project_docs_rate_aware.py) performs the fail-closed 64-organization Project and documentation reconciliation.
- [`scripts/ops/sync_org_project_docs.sh`](../../scripts/ops/sync_org_project_docs.sh) is the lower-level organization reconciler used by the rate-aware controller.

The persistent validation workflow is [`.github/workflows/validate-portfolio-project-links.yml`](../../.github/workflows/validate-portfolio-project-links.yml). Fleet-wide organization reconciliation must use an authenticated, rate-aware execution path with sufficient organization and Projects permissions. Credentials belong in the protected environment or process environment; never place a PAT in command-line arguments, registry files, documentation, evidence, or workflow inputs.

## Adding or changing an organization

1. Update the 64-organization governance TSV in case-insensitive sorted order.
2. For an active portfolio, update the 41-row CSV with the exact GitHub Project, Linear UUID/URL, and Slack IDs.
3. Run the cross-registry and provider-specific validators.
4. Reconcile provider metadata through the reviewed workflow.
5. Commit only validated, redacted evidence. Preserve failed evidence as an explicitly quarantined incident instead of rewriting it to appear successful.
