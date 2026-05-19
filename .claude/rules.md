# Claude Instructions

You are authorized to use all commands (whitelisted), EXCEPT for the following commands which are explicitly BLACKLISTED:

- `rm`
- `mv`
- `git stash`
- `git checkout` (except for switching to a tag/sha inside a submodule; ask the user before any branch switch)
- `git rebase` (including `git pull --rebase`, which invokes a rebase under the hood)
- `sed`

Do not run, propose, or suggest these commands under any circumstances.
