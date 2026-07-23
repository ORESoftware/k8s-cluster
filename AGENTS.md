# Agent rules — scintilla-run-monorepo

This private git superproject is the reviewed integration and GitOps deployment
authority for the scintilla-run fleet. Application source remains canonical in
each repository pinned under `apps/`.

## Safety and history

- Keep history append-only. Never rebase, reset, force-push, rewrite history,
  clean the worktree, or discard uncommitted work.
- Resolve merges semantically and preserve the intended behavior of both sides.
- Do not delete branches, tags, tracked files, or worktrees merely to simplify a
  merge. Stage reviewed removals explicitly and explain them in the commit.
- Integrate remote commits before pushing. Sync means fetch, merge, and push.

## Repository boundaries

- Every `apps/*` entry is a gitlink to an application repository's `main`.
- Change application code in its standalone repository first, push it, and only
  then update the corresponding pin here.
- `scintilla-run.github.io` is intentionally outside the monorepo because Pages
  deploys independently.
- Production deployment is owned here. Application-repository Actions test and
  publish images only; they do not hold cluster credentials or mutate GitOps
  desired state.
- `scintilla-run-infra` owns Cloudflare and Kubernetes app-of-apps manifests.

## Build context

The monorepo is itself pinned in `ORESoftware/k8s-cluster` at
`remote/deployments/scintilla-run-monorepo`. Full integration builds run in that
checkout so application path dependencies can reach `remote/libs` and
`remote/submodules`. Do not edit the nested k8s checkout as source of truth.

## Required checks

Run `npm test` before committing. Deployment changes must also render and
validate the `scintilla-run-infra` Kubernetes overlays without contacting a
cluster.
