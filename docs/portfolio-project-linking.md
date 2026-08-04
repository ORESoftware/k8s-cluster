# Portfolio project linking

The canonical join key across ChatGPT Projects, GitHub organization Projects v2,
Linear projects, and Slack channels is `portfolio_key`.

## Naming and identity contract

- `portfolio_key`: lowercase GitHub organization login, preserving punctuation.
- ChatGPT project name: `<portfolio_key>`.
- Slack channel: `#<portfolio_key>`.
- GitHub Project title: `<GitHub organization login>-project`.
- Linear project: the existing canonical project identified by
  `linear_project_id`; `github.com/<org>` is preferred, while established
  project names remain valid aliases.

The machine-readable source of truth is
[`ops/registries/portfolio-project-links.csv`](../ops/registries/portfolio-project-links.csv).
It contains all 41 verified mappings, source-native IDs, and direct provider
URLs. `dancing-dragons-project` is Project `#4`; every other organization
Project is `#1`.

Every cross-system issue, event, agent run, Slack command, Project item, and
synchronization record should carry `portfolio_key=<key>` plus its source-native
IDs. Matching uses `portfolio_key` first and immutable IDs second; casing
differences must never create duplicate projects or channels.

GitHub organization casing is preserved in `github_org` and
`github_project_title`. The canonical key normalizes historical casing such as
`3FA-app`, `OmniBlitz`, and `StreemPilot` to `3fa-app`, `omniblitz`, and
`streempilot`.

ChatGPT project IDs are not exposed by the available connector, so ChatGPT
linkage is maintained by the exact canonical project name/key stored in the
registry and repeated in GitHub, Linear, and Slack metadata. An optional
agent-bridge/coordinator webhook can perform native ChatGPT-project writes when
that internal capability is available.

## Daily reconciliation

`.github/workflows/validate-portfolio-project-links.yml` performs one active
reconciliation every day at **03:00 America/Chicago**.

GitHub Actions cron is UTC and cannot express a daylight-saving-aware named
timezone. The workflow therefore registers both `08:00` and `09:00` UTC. The
reconciler evaluates the cron expression carried by the schedule event and only
the lane that maps to 03:00 Central is active; the other lane records a clean
no-op. The guard uses the event expression rather than runner start time, so an
ordinary queue delay cannot skip the run.

The reconciler is idempotent and fail-closed:

- **GitHub:** resolve the exact organization Project number, verify its direct
  URL, enforce the canonical title and open state, and maintain a compact
  managed cross-link marker in the Project short description.
- **Linear:** fetch the immutable project ID, verify the canonical name and URL,
  and append or replace a bounded managed link block while preserving all
  human-authored description text.
- **Slack:** verify the authenticated workspace and immutable channel ID, reject
  renamed or archived channels, and maintain a bounded managed cross-link
  marker in the channel topic while preserving the existing human topic.
- **ChatGPT:** always validate the exact project name/key. When
  `CHATGPT_PROJECT_SYNC_ENDPOINT` is configured, send the complete versioned
  routing payload to the agent bridge/coordinator; otherwise report
  `registry_only` rather than claiming a native write occurred.

Each run emits retained JSON and Markdown evidence. Changed or failed active
runs post a portfolio summary to `#oresoftware`.

[`scripts/ops/sync_github_project_metadata.py`](../scripts/ops/sync_github_project_metadata.py)
remains the focused GitHub-only repair/audit command used for the initial
portfolio rollout. The daily reconciler emits the same GitHub short description
and Project readme format, then extends reconciliation to Linear, Slack, and the
optional ChatGPT bridge.

This job synchronizes project identity, direct links, and managed routing
metadata. It deliberately does not duplicate arbitrary message history or
create a second issue system. Work-item mirroring between Linear and GitHub
Projects should consume the same `portfolio_key` registry as a separate,
reviewable policy layer.

Native Linear-to-Slack channel binding remains controlled by Linear's Slack
integration surface. The daily reconciler validates the same immutable IDs and
canonical key on both sides, but does not claim to perform an API write that
Linear does not expose.

## Protected credentials

Configure these as repository Actions secrets. An environment may be used only
when it does not require interactive approval for scheduled runs:

| Secret | Purpose |
|---|---|
| `PROJECT_SYNC_GITHUB_TOKEN` | Read and update organization Projects v2 across the 41 organizations. Prefer a GitHub App token or a rotated least-privilege PAT. |
| `LINEAR_API_KEY` | Read canonical projects by ID and update their managed description blocks. |
| `SLACK_BOT_TOKEN` | Verify channels, update topics, and post the portfolio summary. |
| `CHATGPT_PROJECT_SYNC_ENDPOINT` | Optional internal agent-bridge/coordinator endpoint for native ChatGPT-project reconciliation. |
| `CHATGPT_PROJECT_SYNC_TOKEN` | Optional bearer credential for the ChatGPT bridge endpoint. |

The apply path exits before provider calls when the active lane lacks any of the
three required GitHub, Linear, or Slack credentials. Pull requests receive no
provider secrets and run validation, unit tests, and a credential-free plan
only.

A token pasted into chat must be treated as exposed: rotate it, install only the
replacement as a protected secret, and never commit it or place it in a
credential helper used by automation.

## Validation and manual operation

```console
python scripts/ops/validate_portfolio_project_links.py
python -m unittest discover -s scripts/ops/tests -v
python scripts/ops/sync_portfolio_project_links.py \
  --allow-missing-credentials
```

Apply mode uses the protected environment variables above:

```console
python scripts/ops/sync_portfolio_project_links.py --apply
```

Manual GitHub Actions dispatch defaults to validation-only. Set the `apply`
input explicitly to perform provider writes.
