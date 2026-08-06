# GitHub and Linear projects by organization

This registry connects repository ownership, GitHub delivery tracking, and
Linear product planning. Add one row per GitHub organization and keep links
stable when repositories move within that organization.

| GitHub organization | GitHub Project | Linear project |
| --- | --- | --- |
| `canonical-cloud` | [Canonical project board](https://github.com/orgs/canonical-cloud/projects/1) | [Canonical GitHub project](https://linear.app/denman/project/githubcomcanonical-cloud-1659c8ea1adf) |

## Working agreement

- GitHub issues and pull requests are the source of truth for repository-level
  implementation, review, immutable SHAs, and release artifacts.
- The organization GitHub Project is the cross-repository delivery view.
- Linear is the product-planning view. Link each Linear work item to its GitHub
  issue or pull request instead of copying implementation detail.
- Infrastructure work records the target environment and every activation gate,
  especially external secrets, database migrations, image digests, Cloudflare
  DNS and Worker routes, and rollback revisions.
