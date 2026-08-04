# Portfolio project sync

## Purpose

`portfolio-project-sync.yml` is the single scheduled reconciler for the ORESoftware
portfolio identity graph. It runs every day at **03:00 in `America/Chicago`** and
keeps the canonical project key connected across:

- an organization-level GitHub Projects v2 board;
- one canonical Linear project;
- one ChatGPT Project name;
- one public Slack channel in the `oresoftware` workspace.

The checked-in registry at `config/portfolio-projects.json` contains exactly 41
entries. Every system is addressed by an immutable ID where the platform exposes
one, while the shared lower-case key provides the human-readable join key.

## Scheduling contract

The workflow has one production scheduler:

```yaml
schedule:
  - cron: "0 3 * * *"
    timezone: "America/Chicago"
```

Using a timezone-aware GitHub Actions schedule keeps the local execution time at
03:00 across Central Time daylight-saving transitions. There is no second
Kubernetes CronJob for the same operation; one scheduler avoids duplicate writes
and split-brain reconciliation.

The workflow also supports a manual invocation. Manual runs default to dry-run,
while scheduled runs always use `--apply --sync-issues`.

## Canonical registry

Each registry entry has this shape:

```json
{
  "key": "zed-pkg",
  "chatgpt": {
    "name": "zed-pkg"
  },
  "github": {
    "owner": "zed-pkg",
    "project_number": 1,
    "project_title": "zed-pkg-project",
    "project_url": "https://github.com/orgs/zed-pkg/projects/1"
  },
  "linear": {
    "project_id": "9107ce62-1112-43ff-89bc-f442613c4156",
    "project_name": "github.com/zed-pkg"
  },
  "slack": {
    "workspace_id": "T01B3C83PMK",
    "channel_id": "C0BL0K0HABB",
    "channel_name": "zed-pkg"
  },
  "issue_sync": {
    "enabled": true,
    "source": "linear",
    "target": "github-project-draft-items"
  }
}
```

Registry validation is intentionally strict:

- exactly 41 mappings are required;
- canonical keys, GitHub owners, Linear project IDs, Slack channel IDs, Slack
  names, and ChatGPT names must each be unique;
- ChatGPT Project and Slack channel names must equal the canonical key;
- the GitHub Project title must be `<owner>-project`;
- all GitHub Projects must be Project #1 except `dancing-dragons`, which is
  Project #4;
- the cron and timezone cannot drift from the requested schedule.

Case-sensitive GitHub owners are retained where required, including `3FA-app`,
`OmniBlitz`, and `StreemPilot`. Their canonical keys remain lower case.

## Reconciliation behavior

### GitHub Projects v2

For each board, the reconciler reads the project and all project items, then
reconciles only these project-level fields:

- title;
- open/closed state, always reopening the canonical board when necessary;
- short description containing the cross-system key and IDs;
- one marked identity block in the project README.

Text outside the marked README block is preserved. A missing or duplicate marker
causes the entry to fail closed rather than replacing human-authored content.

### Linear

Linear is the source of truth for work items. The reconciler uses the exact
project UUID from the registry and reconciles:

- the canonical Linear project name;
- one marked cross-system identity block in the project description;
- active issue metadata into managed GitHub Project draft items when issue sync
  is enabled.

Canceled duplicate Linear projects are not selected by name and are never used;
the UUID in the registry is authoritative.

### Slack

The reconciler looks up every channel by its immutable Slack channel ID, not by
name. It then reconciles:

- the lower-case canonical channel name;
- one bounded managed identity segment in the channel purpose.

Existing human purpose text is preserved. Slack purposes are capped at 250
characters, so the reconciler first tries full links and then a compact ID form.
It refuses to truncate human text. Public channels are joined automatically only
when Slack requires membership before a purpose update.

### ChatGPT Projects

The canonical ChatGPT Project name is always present in the registry. The
workflow does not scrape ChatGPT or mutate consumer ChatGPT Projects through an
undocumented endpoint.

Live ChatGPT verification is optional and read-only. A supported export or bridge
can write a JSON snapshot with this shape:

```json
{
  "projects": [
    { "name": "zed-pkg" },
    { "name": "sonus-auris" }
  ]
}
```

The complete example is
`config/chatgpt-projects.snapshot.example.json`. Store the JSON as the optional
repository secret `CHATGPT_PROJECTS_SNAPSHOT_JSON`. When supplied, every registry
entry must exist in the snapshot. A manual run can set
`require_chatgpt_snapshot=true` to fail when the snapshot is absent or
incomplete.

This provides a clean boundary for a future official ChatGPT Projects export,
Compliance API integration, or an authenticated internal bridge without binding
the core reconciler to browser automation.

## Linear issue mirror

The issue mirror is deliberately one-way:

```text
Linear project issues -> managed GitHub Project draft items
```

Pending Linear workflow state types are `triage`, `backlog`, `unstarted`, and
`started`.
Completed and canceled issues are terminal.

Each managed draft contains an invisible marker with the Linear issue UUID:

```html
<!-- portfolio-sync:linear-issue-id:11111111-1111-1111-1111-111111111111 -->
```

The reconciler:

1. creates a draft item for an active Linear issue without a mirror;
2. updates the title/body when Linear metadata changes;
3. archives the managed draft when the Linear issue becomes terminal, leaves the
   canonical project, or disappears from the project query;
4. never modifies real GitHub Issues, pull requests, or unmarked draft items;
5. fails closed when duplicate managed drafts point to the same Linear issue.

The mirrored body includes the Linear identifier, URL, state, priority, and
`updatedAt` value. Work should still be edited in Linear.

## Required secrets

Configure these repository or protected-environment secrets in
`ORESoftware/k8s-cluster`:

| Secret | Purpose | Minimum capability |
|---|---|---|
| `PORTFOLIO_GITHUB_TOKEN` | GitHub GraphQL queries and Project v2 mutations across the 41 organizations | Read/write access to each organization Project |
| `LINEAR_API_KEY` | Linear GraphQL project reads and project-description/name updates | Access to the workspace and project update permissions |
| `SLACK_BOT_TOKEN` | Channel inventory, rename, join, and purpose update | Public channel read/join plus channel manage/topic permissions |
| `CHATGPT_PROJECTS_SNAPSHOT_JSON` | Optional read-only verification input | No external credential; JSON project-name snapshot only |

The previously pasted GitHub PAT must not be reused. Rotate it and store only the
replacement as a GitHub secret. Tokens are read from environment variables and
are never included in URLs, reports, artifacts, or log messages.

The scheduled workflow uses `permissions: contents: read`; cross-organization
writes come only from the explicit `PORTFOLIO_GITHUB_TOKEN`, not the workflow's
repository-scoped `GITHUB_TOKEN`.

## Safety boundaries

The reconciler is idempotent and bounded:

- one workflow-wide concurrency group prevents overlapping production runs;
- API requests retry bounded 429 and 5xx responses with `Retry-After` support;
- Linear pagination has a configurable maximum issue count per project;
- Slack writes are spaced to stay within the documented method tier;
- each project is isolated so one mapping error does not prevent the remaining
  mappings from being audited;
- any entry-level error makes the workflow fail after a complete JSON report is
  written;
- all mutations are individually recorded as proposed or applied changes;
- secrets are redacted by construction and error responses are reduced to safe
  messages.

The job does **not** synchronize Slack message history, ChatGPT conversation
content, GitHub repository contents, issue comments, attachments, or user data.

## Evidence and observability

Every run writes:

- a Markdown table to the GitHub Actions step summary;
- `artifacts/portfolio-project-sync-report.json` as a retained workflow artifact;
- per-project status, proposed/applied mutations, warnings, errors, and issue
  mirror counts.

Pull requests run a credential-free validation job that:

- compiles the Python source;
- runs the unit-test suite;
- validates all 41 registry mappings;
- uploads a validation report without accessing any external account.

## Manual operations

### Validate locally

```bash
python3 -m py_compile scripts/ops/sync_portfolio_projects.py
python3 -m unittest discover -s tests/ops -p 'test_sync_portfolio_projects.py'
python3 scripts/ops/sync_portfolio_projects.py \
  --validate-only \
  --registry config/portfolio-projects.json \
  --report artifacts/portfolio-project-sync-validation.json
```

### External dry-run

With the three required tokens exported in the shell:

```bash
python3 scripts/ops/sync_portfolio_projects.py \
  --registry config/portfolio-projects.json \
  --sync-issues \
  --fail-on-drift \
  --report artifacts/portfolio-project-sync-report.json
```

Dry-run reads all systems and reports drift without mutation. With
`--fail-on-drift`, exit code `2` means drift was detected; exit code `1` means a
validation or API error occurred.

### Apply

```bash
python3 scripts/ops/sync_portfolio_projects.py \
  --registry config/portfolio-projects.json \
  --apply \
  --sync-issues \
  --report artifacts/portfolio-project-sync-report.json
```

The GitHub Actions manual trigger exposes equivalent `apply`, `sync_issues`, and
`require_chatgpt_snapshot` controls.

## Adding or changing a project

1. Establish the canonical lower-case key.
2. Create or select the GitHub organization Project and record its exact owner,
   number, title, and URL.
3. Select the active Linear project by UUID, not only by display name.
4. Select the Slack public channel by channel ID.
5. Ensure the ChatGPT Project name equals the canonical key.
6. Update `config/portfolio-projects.json` in one commit.
7. Update the ChatGPT snapshot example when the portfolio size changes.
8. Run the credential-free tests and validation.
9. Review a manual dry-run before applying external mutations.

Changing the fleet size requires intentionally changing
`EXPECTED_PORTFOLIO_SIZE` and its tests. This prevents an accidental omission or
silent duplicate from becoming the new source of truth.

## Rollback

Disable the scheduled lane by reverting the workflow commit or disabling the
workflow in GitHub Actions. Existing managed metadata remains readable and no
further writes occur.

To remove only generated issue mirrors, disable `issue_sync.enabled` for a
mapping and run a reviewed cleanup migration. Do not delete unmarked project
items. Identity blocks are marked so they can be removed explicitly while
preserving surrounding human text.
