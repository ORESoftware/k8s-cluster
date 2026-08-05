# Durable worker GitHub Project and Linear operating model

Status: active

Repository: [`ORESoftware/k8s-cluster`](https://github.com/ORESoftware/k8s-cluster)

Integration branch: `dev`

Linear project: `github.com/ORESoftware/k8s-cluster`

Linear roadmap document: [Durable Worker Runtime — roadmap and GitHub Project operating model](https://linear.app/denman/document/durable-worker-runtime-roadmap-and-github-project-operating-model-5da4225d886a)

Baseline issue: [DEN-1675](https://linear.app/denman/issue/DEN-1675/add-independent-durable-worker-runtime-to-k8s-cluster)

## Purpose

This document defines how durable-worker work is represented consistently in GitHub Issues, pull requests, the organization GitHub Project, and Linear. It is intentionally repository-owned so field meanings and completion gates can be reviewed alongside code.

## Canonical ownership

| Concern | Canonical system |
| --- | --- |
| implementation, tests, GitOps, and permanent technical documentation | GitHub repository |
| code review and merge evidence | GitHub pull request |
| milestone planning, blockers, dependencies, and assignee state | Linear |
| portfolio view and cross-repository status | GitHub organization Project |
| protocol and operational acceptance gates | repository docs and executable tests |

A copied title is not a durable link. Every planned item must contain explicit URLs or identifiers for its counterparts.

## Required item lifecycle

### 1. Plan

Create or amend a Linear issue in `github.com/ORESoftware/k8s-cluster` with:

- a concrete outcome rather than an implementation placeholder;
- acceptance criteria;
- milestone and component;
- relation to `DEN-1675`;
- explicit blockers and blocked work;
- risk level and target horizon.

Create a GitHub issue when work is ready to enter the engineering backlog. Prefix its title with `workers:` and include the Linear identifier in the body.

### 2. Implement

Use one focused branch and PR per independently reviewable outcome. The PR body must link the Linear issue and any GitHub issue it closes.

The branch must start from current `dev`. If `dev` moves, either rebase or create a semantic merge that preserves both sides. Do not resolve conflicts by blindly selecting one parent.

### 3. Validate

Before readiness:

- run focused tests locally when the runtime is available;
- run permanent read-only CI on GitHub;
- run secret and credential-propagation checks;
- remove source carriers, archives, self-deleting publishers, and temporary write permissions;
- compare the final branch against current `dev` and inspect the exact file list;
- rerun final-head checks after any merge or rebase.

### 4. Merge

Use expected-head protection when invoking the merge. A moved head must be reviewed and revalidated rather than merged under a stale approval.

Close the GitHub issue and mark the Linear issue Done only after the PR reports `merged: true` and a merge commit SHA is available.

### 5. Record

Update the roadmap and project status when a milestone changes materially. Documentation-only status changes must not claim an unexecuted test or undeployed capability.

## Recommended GitHub Project fields

| Field | Type | Values or format |
| --- | --- | --- |
| Status | single select | Backlog, Ready, In progress, In review, Blocked, Done |
| Milestone | single select | M1 Replay, M2 Composition, M3 SDK fleet, M4 Operations, M5 HA |
| Component | single select | Control plane, Worker SDK, Broker adapter, Build adapter, GitOps, Observability |
| Risk | single select | Low, Medium, High |
| Target | single select | Current, Next, Later |
| Linear | text | `DEN-####` |
| Repository | text | `owner/repository` |
| PR | text | pull request URL |

Suggested views:

1. **Current delivery** — Status is not Done and Target is Current.
2. **Milestone roadmap** — grouped by Milestone, sorted by Risk then Status.
3. **SDK fleet** — Component is Worker SDK, grouped by language label.
4. **Blocked work** — Status is Blocked, with Linear and dependency fields visible.
5. **Recently merged** — Status is Done, sorted by PR merge time.

## Label vocabulary

Use existing repository labels where possible. Durable-worker-specific issue bodies should also carry structured fields even when labels are unavailable:

```markdown
Linear: DEN-####
Milestone: M1 Replay
Component: Control plane
Risk: High
Target: Current
```

Recommended labels:

- `workers`;
- `durable-execution`;
- `sdk`;
- `gitops`;
- `observability`;
- language labels such as `python`, `typescript`, `go`, `rust`, `dart`, `gleam`, `erlang`.

Do not create near-duplicate labels solely to vary punctuation or capitalization.

## Milestone definitions

### M1 Replay

Resumable event cursors, projections, search, rebuild, and operator read APIs.

### M2 Composition

Schedules, child runs, continue-as-new, definition versions, compensation, and fan-out/fan-in.

### M3 SDK fleet

Lifecycle-aware SDKs, broker/build adapters, examples, and cross-language conformance.

### M4 Operations

Tenant quotas, authorization, audit, OpenTelemetry, operator UI, DLQ, and redaction.

### M5 HA

Fiducia ownership epochs, partitioned streams, failover, chaos testing, and disaster recovery.

## Current milestone record

| Work | Linear | GitHub | State |
| --- | --- | --- | --- |
| independent Rust durable control plane | DEN-1675 | PR #714 | merged |
| destructive restart and fencing proof | related to DEN-1675 | PR #783 | merged |
| TypeScript execution SDK | related to DEN-1675 | PR #791 | merged |
| Python execution SDK | DEN-2218 | PR #971 | in review |

## Automation boundaries

Repository workflows may validate project documentation, issue-body conventions, generated contracts, and links. Permanent pull-request validation workflows must use `contents: read` and check out without persistent credentials.

A narrowly scoped, one-use publication workflow may be used only when direct Git transport is unavailable and all of the following hold:

- exact same-repository branch and PR checks;
- a reviewed checksum and path allow-list;
- tests before publication;
- no plaintext user token;
- all archive pieces and write-enabled workflow removed before readiness;
- final clean-head CI rerun.

The organization Project is an index, not an execution authority. Code and tests remain authoritative for implementation; Linear remains authoritative for planning relationships and status rationale.

## Review checklist

- [ ] Linear issue linked and acceptance criteria present.
- [ ] GitHub issue linked or intentionally omitted because the PR is the first representation.
- [ ] Milestone, component, risk, and target recorded.
- [ ] Exact final file list reviewed.
- [ ] Focused tests green on final head.
- [ ] Secret and no-PAT checks green.
- [ ] Temporary write machinery absent.
- [ ] Current `dev` incorporated semantically.
- [ ] PR merged with expected head SHA.
- [ ] Linear and GitHub project status updated after merge.
