# Daily project-link reconciliation

`catalog/project-links.json` is the public, fleet-wide identity contract for the
41 linked workspaces. Every record has one lowercase canonical key and names the
corresponding GitHub organization Project, Linear project, ChatGPT project, and
Slack channel. Immutable provider IDs and credentials are deliberately absent
from the public repository.

The scheduled workflow is `.github/workflows/daily-project-link-sync.yml`. It
runs once per day at **03:00 America/Chicago**. GitHub Actions schedules in UTC,
so the workflow registers both `08:00` and `09:00` UTC and
`tools/project_links.py` selects the expression that maps to 03:00 Central for
the current daylight- or standard-time offset. The guard evaluates the cron
expression delivered with the event rather than the runner start time, so an
ordinary queue delay does not skip the run.

## Reconciliation behavior

`tools/project_sync.py` is idempotent and fail-closed:

- GitHub: resolve the exact organization Project number, enforce the exact
  `<organization>-project` title, reopen it when closed, and maintain a compact
  canonical-link marker in the Project short description.
- Linear: require one active exact-name project and append or replace a bounded
  managed block in its description without changing human-authored text.
- Slack: require the exact eponymous channel, optionally create it, and maintain
  a bounded canonical-link marker in the channel topic while preserving an
  existing human topic.
- ChatGPT: keep the canonical project name in the public registry and in every
  provider marker. When `CHATGPT_PROJECT_SYNC_ENDPOINT` is configured, send the
  complete versioned catalog to the agent bridge/coordinator webhook for native
  ChatGPT-project reconciliation.

Exact-name ambiguity is an error. Canceled Linear duplicates are ignored, but
two active projects with the same canonical name stop the run. A provider
failure is recorded per project in both JSON and Markdown evidence and makes the
workflow fail.

The current verified GitHub exception is `dancing-dragons-project`, which is
Project `#4`; all other cataloged organization Projects are `#1`.

## Required protected credentials

Configure these as repository or `project-link-sync` environment secrets:

| Secret | Purpose |
|---|---|
| `PROJECT_SYNC_GITHUB_TOKEN` | Read/update organization Projects v2 across the fleet. Use a rotated, least-privilege token; never the token pasted into chat. |
| `LINEAR_API_KEY` | Read active projects and update their managed description blocks. |
| `SLACK_BOT_TOKEN` | List/create project channels, set channel topics, and post the portfolio summary. |
| `CHATGPT_PROJECT_SYNC_ENDPOINT` | Optional internal bridge/coordinator endpoint for native ChatGPT-project reconciliation. |
| `CHATGPT_PROJECT_SYNC_TOKEN` | Optional bearer credential for the ChatGPT-project bridge endpoint. |

The apply job refuses to run when any of the three required provider credentials
is missing. Pull requests run only the credential-free catalog validation,
unit tests, and a dry-run plan, so untrusted changes never receive secrets.

Native Linear-to-Slack channel binding is still managed by Linear's Slack
integration surface. The daily job verifies and publishes the same canonical
key on both sides; it does not publish immutable Slack IDs or Linear UUIDs in
this public repository.

## Local commands

```console
python3 tools/project_links.py validate catalog/project-links.json
(
  cd tools
  python3 -m unittest -v test_project_links.py test_project_sync.py
)
python3 tools/project_sync.py --allow-missing-credentials
```

Apply mode uses the protected environment variables above:

```console
python3 tools/project_sync.py --apply --create-missing-slack
```

A manual GitHub Actions dispatch defaults to validation-only. Select `apply`
explicitly to perform provider writes.
