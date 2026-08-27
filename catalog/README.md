# Canonical repository catalog

DEN-627 defines a deterministic catalog contract for the 24 installed GitHub
owners governed by DEN-598. The catalog separates facts from hypotheses:
metadata-derived classifications are `present-unverified`, unknowns stay in the
review queue, and dependency edges are only `verified` when their source
evidence and immutable pin are recorded.

DEN-630 adds `catalog/applications.json`, the canonical inventory of tracked
Argo CD `Application` declarations under `remote/argocd`. The generated catalog
groups every declaration by `metadata.name`, records duplicate names without
silently choosing a winner, and captures source, destination, project, and sync
policy facts. Its versioned JSON Schema is
`catalog/applications.schema.json`.

The application check is deliberately strict:

- any tracked Application manifest change must regenerate the catalog;
- duplicate names become reviewable catalog drift;
- declarations that tell Argo CD to render a path inside a git submodule fail
  CI; and
- direct upstream application repositories remain valid, keeping submodules
  as inventory pins rather than render roots.

DEN-1267 adds `catalog/channels.json`, the canonical registry binding each
Slack project channel to its eponymous GitHub organization and Linear project.
Its versioned JSON Schema is `catalog/channels.schema.json`.

The registry records a diagnosis rather than a knob. Every tracked Linear
project already enables all three writable Slack toggles (`slackNewIssue`,
`slackIssueComments`, `slackIssueStatuses`), so a silent channel is never a
notification-settings problem. What is missing is `slackChannelId`, and
Linear's `ProjectUpdateInput` exposes no field for it — moving a channel from
`unbound` to `bound` is an operator action in Linear's Slack integration
surface, not an API write. The registry therefore names the remaining manual
work and keeps it auditable.

The channel check enforces:

- every channel is eponymous with its GitHub owner (`#3fa-app` ↔ `3FA-app`);
- no duplicate channel, owner, or Linear project;
- a `bound` channel must have been inventoried; and
- no immutable Slack channel ID or Linear UUID may enter this public
  repository — those identifiers stay on DEN-1267.

## Data boundary

This repository is public. It may contain:

- complete records for public repositories;
- aggregate counts for private repositories; and
- public fixtures that exercise the schema and DEN-369 import contract.

It must not contain private repository names, private dependency edges, or
private DEN-369 records. A full collection therefore requires an explicit flag
and refuses an output path inside this checkout. The approved access-controlled
destination for the confidential overlay is still an operator decision tracked
by DEN-627.

`catalog/repositories.json` is the complete current public-safe snapshot.
`catalog/fixtures/repositories.v2.json` is the small reviewed contract fixture.
`catalog/baselines/2026-07-28.summary.json` preserves the governed aggregate
baseline without leaking private names. Generated current public snapshots are
safe to commit after review; full snapshots are not.

The July 29 public snapshot currently reports 548 total repositories
(387 public records and 161 private aggregate records). Its
`inventory.baseline_deltas` object preserves count-level drift from the July 28
547-repository baseline.

## Nix workflow

Enter the locked environment with:

```console
nix develop
```

Run the complete local gate:

```console
nix develop -c agent-check all
```

Regenerate and verify the Argo CD Application catalog:

```console
nix develop -c python tools/application_catalog.py generate \
  catalog/applications.json
nix develop -c python tools/application_catalog.py check \
  catalog/applications.json
```

Collect the current public-safe inventory:

```console
nix develop -c agent-check collect-public
```

Collect a confidential full overlay only to an approved path outside the
repository:

```console
nix develop -c python tools/repository_catalog.py collect \
  --owners catalog/owners.json \
  --visibility full \
  --allow-private-output \
  --repo-root "$PWD" \
  --output /approved/access-controlled/repository-catalog.json
```

Collection is read-only and uses the authenticated `gh api /user/repos`
endpoint. It filters results to the exact owner contract in
`catalog/owners.json`.

## Drift and dashboard

```console
nix develop -c python tools/repository_catalog.py diff \
  catalog/repositories.json catalog/repositories.json \
  --json-output artifacts/repository-catalog-drift.json \
  --markdown-output artifacts/repository-catalog-drift.md

nix develop -c python tools/repository_catalog.py dashboard \
  catalog/repositories.json \
  --json-output artifacts/repository-catalog-dashboard.json \
  --markdown-output artifacts/repository-catalog-dashboard.md
```

Drift covers additions, removals, canonical-location or ownership moves,
default-branch changes, dependency pin changes, conformance regressions,
classification changes, and Zed package evidence. Dashboard actions route to
the owning Linear conformance issue; client/SDK Zed gaps also route to DEN-637.

## Zed package contract

Client and SDK candidates are required to carry a package contract compatible
with [github.com/zed-pkg](https://github.com/zed-pkg). The catalog records
evidence states for the manifest, lock, immutable source pin, and CI gate.
Name-based discovery only marks a candidate and routes it to DEN-637; it does
not claim that a repository is conformant until those four fields have reviewed
evidence.

## DEN-369 import

The importer consumes the `nix-fleet-audit/report.json@v1` array emitted by
DEN-369 and records its SHA-256:

```console
nix develop -c python tools/repository_catalog.py merge-den369 \
  catalog/fixtures/repositories.v2.json catalog/fixtures/den369-report.json \
  --source-path catalog/fixtures/den369-report.json \
  --output artifacts/repository-catalog-with-den369.json
```

Validation checks the recorded artifact hash when `--repo-root` is supplied.
The fixture proves the contract without publishing any private repository data.
