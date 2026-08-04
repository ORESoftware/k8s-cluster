# Portfolio project linking

The canonical join key across ChatGPT Projects, GitHub organization Projects v2, Linear projects, and Slack channels is `portfolio_key`.

## Naming contract

- `portfolio_key`: lowercase GitHub organization login, preserving punctuation.
- ChatGPT project name: `<portfolio_key>`.
- Slack channel: `#<portfolio_key>`.
- GitHub Project title: `<GitHub organization login>-project`.
- Linear project: the existing canonical project identified by `linear_project_id`; `github.com/<org>` is preferred, while established project names remain valid aliases.

The machine-readable source of truth is [`ops/registries/portfolio-project-links.csv`](../ops/registries/portfolio-project-links.csv). Each row stores native names, IDs, Project numbers, and direct URLs for all connected systems.

Every cross-system issue, event, agent run, Slack command, Project item, and synchronization record should carry `portfolio_key=<key>` plus its source-native IDs. Matching must use `portfolio_key` first and normalized aliases second; casing differences must never create duplicate projects or channels.

GitHub organization casing is preserved in `github_org` and `github_project_title`. The canonical key normalizes historical casing such as `3FA-app`, `OmniBlitz`, and `StreemPilot` to `3fa-app`, `omniblitz`, and `streempilot`.

ChatGPT project IDs are not exposed by the available connector, so ChatGPT linkage is maintained by the exact canonical project name/key stored in the registry and repeated in GitHub, Linear, and Slack metadata.

## Enforcement

- [`scripts/ops/validate_portfolio_project_links.py`](../scripts/ops/validate_portfolio_project_links.py) enforces the exact 41-key inventory, required fields, canonical casing and aliases, Project numbering, UUIDs, the Slack workspace, direct URL consistency, uniqueness, ordering, and credential exclusion.
- [`.github/workflows/validate-portfolio-project-links.yml`](../.github/workflows/validate-portfolio-project-links.yml) runs the validator on relevant pushes and pull requests.
- [`scripts/ops/sync_github_project_metadata.py`](../scripts/ops/sync_github_project_metadata.py) reconciles every GitHub Project readme and short description from the registry.
- Linear projects and Slack channels carry the marker `portfolio-link-registry:v1:<portfolio_key>` with reciprocal links. This marker is the idempotency key: update the existing reference for a portfolio instead of creating another comment or message.

All GitHub organization Projects use Project #1 except the established `dancing-dragons` Project #4.

When adding another portfolio, add or select the Linear project and Slack channel first, then append one sorted registry row, extend the validator inventory, create or update the reciprocal references, and run both validation and metadata synchronization. Never infer a match solely from display text when a native ID is available.
