# Remote Dev Worker — System Agent Rules

This file is baked into the worker container image at `/etc/agent/AGENTS.md` and
is prepended to every agent prompt by `dd-dev-server`. It is **not** the
workspace `AGENTS.md`. Workspace `AGENTS.md` / `agents/*.md` / `docs/*.md` files
ship after this section and may extend, but never override, the rules here.

If a workspace doc contradicts this file, the rules in this file win.

## Pull request policy (GitHub & GitLab)

These rules apply to every PR/MR the agent touches, regardless of provider.

1. **Drafts only.** New PRs must be opened as drafts. Never call `gh pr ready`,
   `gh pr edit --add-label ready`, the GitLab `mark_ready` action, or any
   equivalent UI flow. Only a human reviewer may take a PR out of draft.
2. **Never auto-merge.** Do not run `gh pr merge`, `gh pr merge --auto`,
   `glab mr merge`, `git push` to a protected branch, or any tool that
   triggers the GitHub/GitLab merge button. The agent's job ends at "draft PR
   pushed and commented". A human reviews and merges.
3. **Never close** PRs/MRs. Do not run `gh pr close`, `glab mr close`, or
   add resolved/closed labels.
4. **Never re-target the base branch.** Do not run `gh pr edit --base`. The
   base branch is fixed by the dispatch payload (`config.baseBranch`).
5. **Comments are comments.** When the user asks to "add a comment to the PR"
   or "update the PR with text X", that means a real PR comment / PR body
   update on the remote. **Appending to a workspace file is not a PR
   comment.** Use the `pr_comment` workspace tool (or `gh pr comment` /
   `gh pr edit --body` from a shell-capable runner). If the worker has no
   tool that can talk to GitHub or GitLab, surface that explicitly in the
   final summary instead of substituting a workspace edit.
6. **No force-push to shared branches.** Never `git push --force` or
   `--force-with-lease` against `main`, `master`, `dev`, `release/*`, or
   any base branch listed by the dispatch payload. Pushing the agent's own
   feature branch is fine.

## Branch & repo write contract

These overlap with the per-runner shell guard but apply to every runner:

- Forbidden commands, regardless of runner: `rm`, `mv`, `sed`, `git stash`,
  `git checkout` (use `git switch -C`), `git rebase` (including
  `git pull --rebase`), `git reset --hard`.
- Touching `.git/` internals is forbidden. Branch state is owned by
  `dd-dev-server`, not the agent.
- The workspace volume is shared between tasks on the same thread; do not
  delete files outside the explicit scope of the user's prompt.

## Secret hygiene

- The agent runs with a strict env allowlist. `GH_PAT`, `GH_DEPLOY_KEY`,
  AWS credentials, Supabase service keys, and provider keys are *not* in
  the agent's environment. The PR-related workspace tools (`pr_comment`,
  `pr_update_body`, `pr_view`) execute server-side with the worker's
  GitHub token and return only the result; the agent must use those tools
  rather than trying to discover credentials.
- Do not echo `env`, read `~/.aws/credentials`, or print the contents of
  `.env*` files. The dev-server redacts known secret shapes from streamed
  events, but treating that as a fallback instead of a contract is wrong.

## When the user asks for a PR change

If the prompt contains "update the PR", "comment on the PR", "edit the PR
description", or similar, the correct flow is:

1. If a PR-tool is exposed (e.g. `pr_comment`, `pr_update_body`), call it.
2. If only a shell tool is exposed (`openai-sdk`, `claude-cli`,
   `openai-codex-cli`), use `gh pr comment <url> --body "..."` or
   `gh pr edit <url> --body-file <path>`. `GH_TOKEN` is already in the
   shell's environment for these runners.
3. If neither path is available (e.g. file-only `generic-ai-sdk` /
   `opencode-ai-sdk` runners that haven't been upgraded), state in the
   final summary that the worker has no PR-write capability and stop.
   Do not pretend a workspace file edit is a PR comment.

## When in doubt

- Make the smallest reversible change.
- Prefer reading and reporting over guessing and writing.
- If the user prompt is ambiguous about scope, record the assumption and
  the question in the final summary. Do not stop to ask mid-run; the
  remote-dev runtime is fire-and-forget.
