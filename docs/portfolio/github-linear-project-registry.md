# GitHub organization, Linear, and GitHub Project registry

The machine-readable registry is [`ops/portfolio/github-linear-project-registry.tsv`](../../ops/portfolio/github-linear-project-registry.tsv). It contains the canonical GitHub organization login and Linear project URL for the current 64-organization fleet.

## Operating contract

- GitHub repositories, pull requests, commits, CI, releases, and deployable artifacts are authoritative in GitHub.
- Product planning, dependencies, ownership, milestones, and status are authoritative in the linked Linear project.
- Each organization has one canonical active GitHub Project titled `<canonical-org-login>-project`.
- The canonical Project is normally project `1`. `dancing-dragons` retains its pre-existing canonical project `4`.
- Organization-level documentation lives in the public `<org>/.github` repository.
- The organization Project contains a durable governance issue linking GitHub, Linear, and the organization documentation.
- Documentation conflicts are resolved semantically against the latest default branch. Managed routing blocks are regenerated while unrelated prose is preserved; automation must never resolve conflicts by blindly choosing one side.

## Reconciliation

Run [`scripts/ops/sync_org_project_docs.sh`](../../scripts/ops/sync_org_project_docs.sh) with an authenticated GitHub CLI session that can administer the listed organizations. The one-time workflow [`ops-sync-org-project-docs-once.yml`](../../.github/workflows/ops-sync-org-project-docs-once.yml) performs the fleet-wide reconciliation and publishes machine-readable and Markdown evidence under `ops/evidence/org-project-docs/`.
