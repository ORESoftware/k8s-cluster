# GitHub organization, Linear, and GitHub Project registry

`github-linear-project-registry.tsv` is the version-controlled identity map for the current organization fleet.

## Columns

| Column | Contract |
| --- | --- |
| `organization` | Exact case-sensitive GitHub organization login |
| `github_project_title` | Exact canonical title `<organization>-project` |
| `github_project_url` | Canonical organization Project URL; normally project `1` |
| `linear_url` | Canonical Linear project URL for planning and status |

`dancing-dragons` retains its pre-existing canonical GitHub Project `4`. All other rows currently use project `1`.

## Authority boundaries

- GitHub is authoritative for repositories, commits, pull requests, reviews, checks, releases, and deployable artifacts.
- Linear is authoritative for planning, ownership, dependencies, milestones, and status.
- The organization GitHub Project is the cross-repository execution view; it does not replace GitHub delivery evidence or Linear planning state.
- Public `<org>/.github` repositories are the organization-local documentation entry points.

## Update rules

- Preserve the exact case of organization logins, including `3FA-app`, `OmniBlitz`, and `StreemPilot`.
- Keep one row per organization and one unique Linear URL per row.
- Do not invent a Project number or Linear URL. Verify it from the canonical systems before editing.
- Reconcile documentation semantically from current default branches; do not overwrite unrelated prose.
- Never put tokens, private keys, private message content, or private repository inventories in this registry.

Run the fail-closed validator before publishing:

```sh
python3 ops/portfolio/validate_github_linear_project_registry.py
```

The human-readable Linear mirror is `GitHub organization, Linear, and GitHub Project registry` in the Denman workspace.
