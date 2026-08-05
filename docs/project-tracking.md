# Daedalus organization project tracking

## Canonical systems

- GitHub organization: `daedalus-fab`
- Repository for this integration: `daedalus-fab/fabrication-server.rs`
- Linear project: `github.com/daedalus-fab`
- Linear issue: `DEN-2465`
- Intended GitHub organization project: `daedalus-fab-project`

## Meshy integration work item

The Meshy provider belongs to the fabrication-server workstream because it creates candidate design geometry that must flow through Daedalus design-import, geometry, manufacturability, and release gates.

The pull request should be linked to `DEN-2465`. After merge, the Linear issue should record the merge commit and validation results, then move to Done. The GitHub issue or pull request should be represented in the organization project under the same work item rather than duplicated into a second planning project.

## Status convention

| Delivery state | Linear | GitHub project |
| --- | --- | --- |
| Branch and implementation active | In Progress | In progress |
| Pull request open and checks running | In Review | In review |
| Pull request merged and docs updated | Done | Done |
| Provider credential or external account missing | In Progress with blocker comment | Blocked field or status note |

## Conflict policy

Resolve conflicts semantically using the complete Daedalus fabrication and release context. Do not mechanically choose “ours” or “theirs,” and do not weaken authentication, provenance, machine-readiness, or formal release gates merely to make a merge clean.
