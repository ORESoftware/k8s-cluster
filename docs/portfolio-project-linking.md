# Portfolio project linking

The canonical join key across ChatGPT Projects, GitHub organization Projects v2, Linear projects, and Slack channels is `portfolio_key`.

## Naming contract

- `portfolio_key`: lowercase GitHub organization login, preserving punctuation.
- ChatGPT project name: `<portfolio_key>`.
- Slack channel: `#<portfolio_key>`.
- GitHub Project title: `<GitHub organization login>-project`.
- Linear project: the existing canonical project identified by `linear_project_id`; `github.com/<org>` is preferred, while established project names remain valid aliases.

The machine-readable source of truth is [`ops/registries/portfolio-project-links.csv`](../ops/registries/portfolio-project-links.csv).

Every cross-system issue, event, agent run, Slack command, Project item, and synchronization record should carry `portfolio_key=<key>` plus its source-native IDs. Matching must use `portfolio_key` first and normalized aliases second; casing differences must never create duplicate projects or channels.

GitHub organization casing is preserved in `github_org` and `github_project_title`. The canonical key normalizes historical casing such as `3FA-app`, `OmniBlitz`, and `StreemPilot` to `3fa-app`, `omniblitz`, and `streempilot`.

ChatGPT project IDs are not exposed by the available connector, so ChatGPT linkage is maintained by the exact canonical project name/key stored in the registry and repeated in GitHub, Linear, and Slack metadata.
