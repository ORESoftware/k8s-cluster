# Git hooks (`.githooks/`)

Versioned hooks for this repo. They come in two flavours:

- **Reminders** (`post-merge`, `post-checkout`, `post-rewrite`) — never mutate
  anything; they just nag when a submodule pointer moved but your working tree
  didn't follow.
- **Guard** (`pre-push`) — can **fail** a push. It refuses to publish a submodule
  pointer that the submodule's own remote doesn't have.

## Activation (required, once per clone)

Git does **not** auto-run a tracked hooks dir, so every clone — including the
sync bot's — must opt in once. Without this, neither the reminders nor the guard
fire:

```sh
./.githooks/install.sh        # sets core.hooksPath -> .githooks (idempotent)
```

## The reminders

After any operation that can move a submodule pointer (`pull`/`merge`, branch
`checkout`/`switch`, `rebase`/`amend`), the hook runs
[`submodule-sync-reminder.sh`](submodule-sync-reminder.sh), which checks
`git submodule status` and — if any of this repo's 15 submodules are out of
sync — prints a reminder to run:

```sh
git submodule update --init --recursive
```

This matters because several deployments consume their engines/libs as
submodules (e.g. `dd-des-rs` pulls in `soccer-sim-game-engine.rs` and
`discrete-event-system.rs`), so a stale checkout silently builds an old version.

| Hook            | Fires after                            |
| --------------- | -------------------------------------- |
| `post-merge`    | `git pull` / `git merge`               |
| `post-checkout` | `git checkout` / `git switch` (branch) |
| `post-rewrite`  | `git rebase` / `git commit --amend`    |

To silence the reminder in scripted/non-interactive contexts:

```sh
export DD_SKIP_SUBMODULE_REMINDER=1
```

## The push guard

[`submodule-push-guard.sh`](submodule-push-guard.sh) (run by `pre-push`) is the
fix for the failure mode where the parent records a submodule commit that was
never pushed to the submodule's remote. The parent then references a commit
nobody can fetch and **every** clone's pull breaks with:

```
fatal: remote error: upload-pack: not our ref <sha>
```

For each submodule gitlink the push would introduce or move (diffed against what
the remote already has), the guard fetches that submodule remote's branch/tag
tips and verifies the gitlink commit is reachable from one of them. If not, the
push is **refused** with the exact submodule to push first:

```sh
( cd <submodule> && git push origin HEAD )   # then re-push this repo
```

- Only submodules whose pointer actually changed are checked, so the network
  cost is proportional to the change, not to all 15 submodules.
- The guard never mutates your tree or the submodules (it cleans up its own
  temporary refs).

> **Sync bot:** the bot MUST push each changed submodule before pushing this
> superproject, and must run with the guard active (do **not** pass
> `git push --no-verify`, which skips it). The guard is what turns a silent
> repo-wide breakage into a loud, local "push the submodule first".

Emergency opt-out (reintroduces the failure mode — avoid):

```sh
DD_SKIP_SUBMODULE_PUSH_GUARD=1 git push ...
```
