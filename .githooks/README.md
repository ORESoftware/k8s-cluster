# Git hooks (`.githooks/`)

Versioned hooks for this repo. They are **reminders only** — none of them mutate
your working tree or submodules.

## What they do

After any operation that can move a submodule pointer (`pull`/`merge`,
branch `checkout`/`switch`, `rebase`/`amend`), the hooks run
[`submodule-sync-reminder.sh`](submodule-sync-reminder.sh), which checks
`git submodule status` and — if any of this repo's 15 submodules are out of
sync — prints a reminder to run:

```sh
git submodule update --init --recursive
```

This matters here because several deployments consume their engines/libs as
submodules (e.g. `dd-des-rs` pulls in `soccer-sim-game-engine.rs` and
`discrete-event-system.rs`), so a stale checkout silently builds an old version.

| Hook            | Fires after                          |
| --------------- | ------------------------------------ |
| `post-merge`    | `git pull` / `git merge`             |
| `post-checkout` | `git checkout` / `git switch` (branch) |
| `post-rewrite`  | `git rebase` / `git commit --amend`  |

## Activation

Hooks under `.githooks/` are **not** used until git is pointed at them. This is
a one-time, per-clone local setting (git does not auto-run a versioned hooks
dir):

```sh
git config core.hooksPath .githooks
```

To silence the reminder in scripted/non-interactive contexts (CI, the sync bot):

```sh
export DD_SKIP_SUBMODULE_REMINDER=1
```
