# Portfolio project linking

The canonical join key across ChatGPT Projects, GitHub organization Projects v2, Linear projects, and Slack channels is `portfolio_key`.

## Naming contract

- `portfolio_key`: lowercase GitHub organization login, preserving punctuation.
- ChatGPT project name: `<portfolio_key>`.
- Slack channel: `#<portfolio_key>`.
- GitHub Project title: `<GitHub organization login>-project`.
- Linear project: the existing canonical project identified by `linear_project_id`; `github.com/<org>` is preferred, while established project names remain valid aliases.

The machine-readable source of truth is [`ops/registries/portfolio-project-links.csv`](../ops/registries/portfolio-project-links.csv). It records the canonical names, source-native IDs, and direct URLs required to join the four systems without relying on display-name inference.

Every cross-system issue, event, agent run, Slack command, Project item, and synchronization record should carry `portfolio_key=<key>` plus its source-native IDs. Matching must use `portfolio_key` first and normalized aliases second; casing differences must never create duplicate projects or channels.

GitHub organization casing is preserved in `github_org` and `github_project_title`. The canonical key normalizes historical casing such as `3FA-app`, `OmniBlitz`, and `StreemPilot` to `3fa-app`, `omniblitz`, and `streempilot`.

## Live reciprocal references

Each canonical Slack channel has an administrative reference message, and each canonical Linear project has a matching project comment. Both use the stable marker:

```text
project-link:v1:<portfolio_key>
```

Each reference identifies the ChatGPT project name/key, GitHub organization and organization Project, reciprocal Linear or Slack destination, and authoritative registry. Treat this marker as an idempotency key: synchronization tooling must update the existing record for a key rather than create duplicate reference messages or comments.

A reference marker proves routing metadata is present; it does not replace source-native IDs in synchronization payloads. Automation should resolve the row from the registry, validate the native IDs and URLs, and then attach `portfolio_key` to the resulting GitHub, Linear, Slack, or agent event.

## Validation and drift control

Run the validator from the repository root:

```bash
python3 scripts/ops/validate_portfolio_project_links.py
```

The validator enforces the exact 41-key inventory, canonical casing and aliases, unique native IDs and URLs, GitHub Project numbering, the Slack workspace, Linear URL/name consistency, sorted rows, and credential exclusion. All GitHub organization Projects use Project #1 except the established `dancing-dragons` Project #4.

Any inventory change must update the registry, validator inventory, reciprocal references, and relevant ChatGPT project routing in the same change set.

ChatGPT project IDs are not exposed by the available connector, so ChatGPT linkage is maintained by the exact canonical project name/key stored in the registry and repeated in GitHub, Linear, and Slack metadata.
