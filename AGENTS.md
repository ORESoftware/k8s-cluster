# Agent guidelines — t2v-v2t.rs

Rust voice-to-text / text-to-voice / translation platform. Cargo workspace with
two separately-deployed binaries (`t2v-api`, `t2v-web`).

## Command safety — STRICT (all agents MUST follow)

Never run destructive or irreversible shell commands. To remove or move files,
**always go through git** so the change is tracked and recoverable.

**Blacklisted — do NOT run:**
- `rm`, `rm -rf`, `rmdir`, `unlink` — never delete via raw `rm`.
- bulk / indirect deletion: `find … -delete`, `find … -exec rm …`, `xargs rm`.
- raw `mv` of tracked files; truncating a tracked file with `>` or `truncate`.
- `git reset --hard`, `git clean -fdx`, `git checkout -- .` / `git restore .` mass-discard.
- `git push --force` / history rewrites on shared branches (esp. `main`).
- `dd`, `mkfs`, `shred`, recursive `chmod -R` / `chown -R` on broad paths.

**Whitelisted — safe, prefer these:**
- `git rm` / `git rm --cached`, `git mv`, `git restore <path>` (single file), `git revert`.
- Editing via the editor tools, `git add`, `git commit`, `git switch -c`.

## Architecture

- `crates/core` — **hand-rolled DSP, zero deps**. Custom FFTs (naive DFT +
  recursive + iterative radix-2), Goertzel/DTMF, WAV, mu-law, resampling, VAD,
  STFT. Do NOT pull in `rustfft`/`hound`/`num-complex`; the point is the custom
  implementations. Keep the three-way FFT cross-check tests green.
- `crates/llm` — raw-HTTP LLM clients (OpenAI/Gemini/Anthropic). **No provider
  SDKs.** Response extractors are pure fns with offline unit tests.
- `crates/entity`, `crates/migration` — SeaORM. **Use SeaORM, never raw sqlx.**
  The migration crate is SQLite-dev bootstrap ONLY.
- `crates/api`, `crates/web` — the two servers. `web` is MASH
  (maud + axum + seaorm + htmx), streaming stats over a websocket.

## Database

The `t2v` Postgres namespace is owned by the shared `pg-defs` contract
(`k8s-cluster/remote/libs/pg-defs/schema/schema.sql`) and migrated declaratively
by **dpm** — the app never runs DDL against Postgres; it connects with
`search_path=t2v`. When you change the schema, edit pg-defs' `schema.sql` and
regenerate; do not add Postgres migrations here. Avoid IN-list CHECK constraints
(dpm has a known fixed-point bug on them).

## Before committing

```sh
cargo test --workspace
cargo clippy --workspace --all-targets   # keep it warning-clean
cargo fmt --all
```
