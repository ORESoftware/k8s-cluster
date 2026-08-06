# Agent guidelines — sonus-auris-backend.rs

Rust backend for Sonus Auris — the audio-dashcam server.

## Instruction discovery

- Resolve the real path of `$PWD`, walk its ancestors through the filesystem root, and load every readable lowercase `agents.md` in root-to-leaf order.
- Do not search sibling directories. Deduplicate canonical paths, detect symlink cycles, and report unreadable instruction files.
- `AGENTS.md`, `.claude/CLAUDE.md`, `.gemini/GEMINI.md`, and `.openai/AGENTS.md` are compatibility pointers only; lowercase `agents.md` files are canonical.

## Linear mapping

- GitHub organization: `github.com/sonus-auris`.
- Linear project: `github.com/sonus-auris` in the Denman workspace.
- Locate or create the matching Linear issue before substantial work, and record PR links, tests, blockers, and remaining work there.

## Backend invariants

- Supabase Auth is the upstream identity provider; `shared-auth` is the cross-product canonical principal/session broker; Sonus Postgres stores application data.
- Never auto-link identities by email. Provider subject links must remain explicit and namespaced.
- Preserve JWT issuer, audience, expiry, algorithm, JWKS key-use/algorithm, refresh throttling, and confirmed-email checks.
- Keep ownership tables fail-closed under Postgres RLS. Do not grant anonymous CRUD access or weaken append-only consent/telemetry rules.
- Keep service-role, database, object-store, and signing secrets server-only. Never log credentials, authorization headers, tokens, cookies, or recording keys.
- Preserve PostgreSQL-only SQLx feature boundaries and the narrowly scoped RustSec exception; do not enable MySQL or broad default features accidentally.
- Run real Postgres migration/RLS tests, locked Rust tests, clippy, formatting, release builds, and dependency audits before merge.

## Command safety — STRICT

Never run destructive or irreversible shell commands. Remove or move tracked files through git so changes remain recoverable.

- Do not use raw `rm`, recursive deletion, raw `mv` of tracked files, mass truncation, `git reset --hard`, `git clean -fdx`, mass restore, stash destruction, forced ref deletion, or force-pushes.
- Prefer `git rm`, `git mv`, single-path `git restore`, `git revert`, normal `git stash`, focused edits, commits, and feature branches.
- Stop and ask the operator before any genuinely destructive or binding action.

## Syncing with the remote

1. Inventory local branches, worktrees, uncommitted changes, and relevant remote refs. Preserve all valid work.
2. Commit or safely stash intended work before integrating incoming changes.
3. Run `git fetch --all --prune`.
4. Integrate upstream with merge-based history; do not rebase shared work.
5. Resolve conflicts semantically: do not merely choose ours or theirs. Merge the intended behaviors conceptually.
6. Grep for `<<<<<<<`, `=======`, and `>>>>>>>`; repeat review and tests until no conflict markers remain.
7. Run the complete backend security and test matrix.
8. Push the feature branch, merge through a green PR, and verify local and remote `main` contain the same intended commits.

Never `git rebase` or force-push to perform a shared synchronization.
