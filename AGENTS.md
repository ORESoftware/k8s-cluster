# Branch and Worktree Policy

- Work directly on the `main` branch for now unless the operator explicitly requests a feature branch or pull request.
- Before making changes, confirm that the intended base branch is checked out and current.
- Do not create or use Git worktrees unless the operator explicitly asks.
- Merge any existing non-`main` branch into `main` with an intent-preserving merge, resolve conflicts semantically, and continue work on `main`.
- Push completed work to `origin/main` (or merge the explicitly requested PR and verify the result on `origin/main`).
- Preserve existing uncommitted work and stop for operator guidance if moving to `main` cannot be done safely.

## GitHub ↔ Linear coordination

- GitHub org: `fiducia-cloud` — https://github.com/fiducia-cloud
- Linear workspace/team: `denman` / `Denman` (`DEN`)
- Linear team ID: `eb8ab169-5afe-4b6f-9cab-3f2aa3e887dc`
- Linear project: `github.com/fiducia-cloud`
- Linear project ID: `d9e89bd3-19da-47f3-9bf7-6dc8cc910b70`
- Linear project URL: https://linear.app/denman/project/githubcomfiducia-cloud-8fd5e1bec9d3

Every repository in this GitHub org maps to that Linear project unless a nested `AGENTS.md` explicitly names another project. Before non-trivial work, search the project and reuse/update a suitable issue rather than creating a duplicate. If none exists, create one in team `DEN` and this project with repository/GitHub links, context, scope, acceptance criteria, risks, and validation steps. Link branches, commits, and PRs to Linear when practical; keep status and blockers current; file deferred, incomplete, risky, or follow-up work before ending. Never commit Linear/GitHub tokens or other credentials.

## Syncing with remote — authoritative meaning

“Sync with remote,” “sync the org,” “sync all repos,” or “make main branches up to date” means the entire organization-wide process below, not merely `git pull` in the current checkout.

1. Enumerate every public/private repository in `fiducia-cloud`, including repositories absent locally. Identify every local checkout and `git worktree`; explicitly report archived/read-only exceptions.
2. Preserve all work: inspect status, untracked files, stashes, local branches, remote branches, upstream tracking, and every worktree. Never use hard resets, blanket restores, branch/worktree deletion, or force-pushes to discard work.
3. For every writable repo run `git fetch --all --prune --tags`; ensure local `main` tracks `origin/main`; fast-forward when possible and deliberately reconcile divergent main histories.
4. Inspect every local/remote branch and worktree for commits or intended changes not represented in `main`. Integrate all valuable unique work into `main` using an intent-preserving merge, cherry-pick, or careful reimplementation. Do not blindly merge obsolete/generated history just to satisfy ancestry; no intended work may remain absent from `main`.
5. Resolve conflicts semantically after understanding both sides, surrounding code, history, schemas, callers, and tests. Never globally choose “ours” or “theirs,” and never merely remove marker lines.
6. Run relevant formatting, linting, tests, builds, and protocol/integration checks. Run `git diff --check`, then scan the whole worktree with `rg -n --hidden -g '!.git' '^(<<<<<<< .+|=======|>>>>>>> .+)$' .` (or equivalent recursive `grep`) and investigate every match.
7. Review all intended tracked/untracked changes and exclude secrets or unwanted artifacts, then `git add -A`, commit accurately, and publish to `origin/main`. If branch protection requires a PR, push an integration branch, merge the PR, and verify the final commit on `origin/main`. Never force-push `main` without explicit owner authorization.
8. Fetch again and repeat from step 1 until every writable repo has a clean tree, local `main` equals `origin/main`, all intended branch/worktree work is represented in `main`, checks pass or a concrete Linear blocker is filed, and conflict-marker scans are clean.

Do not claim completion if any repository, branch, worktree, failure, or read-only exception was silently skipped. Report the exact final state and link remaining Linear issues and pull requests.