# .github

GitHub configuration for the `fiducia-monorepo` superproject.

- `workflows/` — CI and the manual production deploy pipeline (see that folder's
  README).
- `dependabot.yml` — weekly Dependabot update PRs for the superproject's npm,
  GitHub Actions, and digest-pinned Docker inputs.

These settings apply only to the superproject itself; each app submodule under
`apps/` carries its own `.github` config in its own repo.
