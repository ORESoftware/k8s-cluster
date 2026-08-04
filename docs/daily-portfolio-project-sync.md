# Daily portfolio project-link synchronization

This runbook covers the ongoing reconciliation driven by
[`scripts/ops/sync_portfolio_project_links.py`](../scripts/ops/sync_portfolio_project_links.py)
and [`.github/workflows/validate-portfolio-project-links.yml`](../.github/workflows/validate-portfolio-project-links.yml).

The canonical source of truth remains
[`ops/registries/portfolio-project-links.csv`](../ops/registries/portfolio-project-links.csv).
The job does not create a second registry.

## Schedule

The workflow performs one active reconciliation every day at **03:00
America/Chicago**.

GitHub Actions cron is UTC and cannot express a daylight-saving-aware named
timezone. The workflow registers both `08:00` and `09:00` UTC. The reconciler
checks the schedule expression included in the event and only the lane mapping
to 03:00 Central is active; the other lane records a clean no-op.

The guard evaluates the event expression rather than the runner start time, so
an ordinary queue delay does not skip the daily run.

## Provider behavior

The reconciler is idempotent and fail-closed.

### GitHub Projects v2

For every registry row it resolves the exact organization Project number,
verifies the direct URL, enforces the canonical title and open state, and
maintains the same Project readme and short-description format used by the
initial portfolio rollout.

`dancing-dragons-project` is Project `#4`; every other registered organization
Project is `#1`.

### Linear

The job fetches each project by immutable UUID, verifies its canonical name and
direct URL, rejects canceled or drifted projects, and appends or replaces a
bounded managed link block. Human-authored project description text is
preserved.

### Slack

The job verifies the authenticated workspace and immutable channel ID, rejects
renamed or archived channels, and maintains a bounded
`portfolio-link-registry:v1:<portfolio_key>` marker in the channel topic. The
existing human-authored topic is preserved.

Changed or failed active runs post a bounded summary to `#oresoftware`.

### ChatGPT Projects

The exact ChatGPT project name and `portfolio_key` are always validated and
included in reciprocal provider metadata.

Native ChatGPT-project writes require an internal agent-bridge/coordinator
endpoint. When `CHATGPT_PROJECT_SYNC_ENDPOINT` is configured, the job sends the
complete versioned routing payload to that endpoint. Without it, the report
uses `registry_only`; it does not claim that a native platform write occurred.

## Relationship to issue synchronization

This workflow synchronizes project identity, direct links, and managed routing
metadata. The separately reviewed
[Linear next-steps workflow](../.github/workflows/sync-linear-next-steps-to-org-projects.yml)
handles selected Linear issues and GitHub Project items. Both layers consume the
same canonical `portfolio_key` contract.

Native Linear-to-Slack channel binding remains controlled by Linear's Slack
integration surface. This reconciler validates the matching IDs and key on both
sides but does not claim an API mutation that Linear does not expose.

## Protected credentials

Configure the following as GitHub Actions repository secrets, or in a protected
environment that does not require interactive approval for scheduled runs:

| Secret | Purpose |
|---|---|
| `PROJECT_SYNC_GITHUB_TOKEN` | Read and update organization Projects v2 across the 41 organizations. Prefer a GitHub App token or a rotated least-privilege PAT. |
| `LINEAR_API_KEY` | Read canonical projects by ID and update their managed description blocks. |
| `SLACK_BOT_TOKEN` | Verify channels, update topics, and post the portfolio summary. |
| `CHATGPT_PROJECT_SYNC_ENDPOINT` | Optional internal bridge/coordinator endpoint for native ChatGPT-project reconciliation. |
| `CHATGPT_PROJECT_SYNC_TOKEN` | Optional bearer credential for the ChatGPT bridge endpoint. |

The active apply lane exits before provider calls when any required GitHub,
Linear, or Slack credential is missing. Pull-request jobs receive no provider
secrets and run only validation, offline tests, and a credential-free plan.

Any token pasted into chat must be treated as exposed. Rotate it and install
only the replacement as a protected secret; never commit it or save it in an
automation credential helper.

## Evidence and failures

Every validation or active run emits JSON and Markdown evidence. Validation
artifacts are retained for 14 days; active reconciliation artifacts are
retained for 30 days.

A provider failure is recorded against the affected `portfolio_key` and causes
the active workflow to fail. Ambiguous identities, wrong Project numbers,
provider URL drift, canceled Linear projects, wrong Slack workspaces, renamed
or archived Slack channels, and managed-marker overflow all fail closed.

## Local validation

```console
python scripts/ops/validate_portfolio_project_links.py
python -m unittest discover -s scripts/ops/tests -v
python scripts/ops/sync_portfolio_project_links.py \
  --allow-missing-credentials
```

Apply mode requires the protected environment variables above:

```console
python scripts/ops/sync_portfolio_project_links.py --apply
```

A manual GitHub Actions dispatch defaults to validation-only. Set the `apply`
input explicitly to perform provider writes.
