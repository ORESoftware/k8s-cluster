# GitHub organization, Linear, and GitHub Project registry

The machine-readable registry is [`ops/portfolio/github-linear-project-registry.tsv`](../../ops/portfolio/github-linear-project-registry.tsv). It contains the canonical GitHub organization login and Linear project URL for the current 64-organization fleet. The GitHub organization URL, governance repository, canonical Project title, Project number, and Project URL are derived deterministically from that source.

## Operating contract

- GitHub repositories, pull requests, commits, CI, releases, and deployable artifacts are authoritative in GitHub.
- Product planning, dependencies, ownership, milestones, and status are authoritative in the linked Linear project.
- Each organization has one canonical active GitHub Project titled `<canonical-org-login>-project`.
- The canonical Project is normally project `1`. `dancing-dragons` retains its pre-existing canonical project `4`.
- Organization-level documentation lives in the public `<org>/.github` repository.
- The organization Project contains a durable governance issue linking GitHub, Linear, and the organization documentation.
- Organization names and Linear project URLs are unique, case-insensitively for GitHub organization ownership.
- Registry rows are sorted by canonical organization login and contain no query strings, fragments, embedded credentials, or credential-shaped values.
- Documentation conflicts are resolved semantically against the latest default branch. Managed routing blocks are regenerated while unrelated prose is preserved; automation must never resolve conflicts by blindly choosing one side.

## Derived link contract

For each TSV row with organization `<org>`:

| Resource | Derived value |
| --- | --- |
| Organization | `https://github.com/<org>` |
| Governance repository | `https://github.com/<org>/.github` |
| Project title | `<org>-project` |
| Project URL | `https://github.com/orgs/<org>/projects/1` |
| Linear project | Exact `linear_url` from the registry |

The sole Project-number exception is `dancing-dragons`, whose canonical Project URL ends in `/projects/4`.

## Validation

[`scripts/ci/check-github-linear-project-registry.mjs`](../../scripts/ci/check-github-linear-project-registry.mjs) validates the full registry without network or credentials. It rejects:

- missing, additional, malformed, or unsorted rows;
- duplicate organization ownership, including case variants;
- duplicate or malformed Linear project URLs;
- invalid GitHub organization logins;
- drift in the Project-number exception;
- credential-bearing or ambiguous URLs;
- missing semantic conflict-resolution documentation.

The permanent [`github-linear-project-registry.yml`](../../.github/workflows/github-linear-project-registry.yml) workflow runs the validator, its positive and negative fixtures, whitespace checks, conflict-marker checks, and a credential-shape scan. It publishes the complete derived organization↔Project↔Linear table to the GitHub Actions step summary without mutating any organization or Project.

## Reconciliation

Run [`scripts/ops/sync_org_project_docs.sh`](../../scripts/ops/sync_org_project_docs.sh) with an authenticated GitHub CLI session that can administer the listed organizations. The rate-aware variant performs the same bounded semantic reconciliation when GitHub API capacity is constrained. The publisher may create or reopen the canonical Project, initialize the public `.github` repository, update only managed routing blocks, open or reuse a normal pull request, and attach the durable governance issue to the canonical Project.

Fleet mutation and read-only registry validation are intentionally separate. A green registry check proves the declared mapping is internally coherent; it does not by itself claim that all Projects, repositories, issues, or pull requests were successfully reconciled remotely. Remote evidence remains under `ops/evidence/org-project-docs/` and the corresponding Linear execution ledger.
