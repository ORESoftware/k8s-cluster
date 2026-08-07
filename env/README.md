# Environment files

Secrets for this repo are **committed, encrypted**, with [sops] + [age].

```
env/enc/<name>.env.enc   ciphertext — committed. This is the source of truth.
env/dec/<name>.env       plaintext  — gitignored, mode 0600, disposable.
```

`env/dec` is a build artifact. Delete it whenever you like and regenerate with
`just env-decrypt`; nothing there is authoritative.

## First run on a new machine

```sh
just env-keygen     # creates your age key (never overwrites an existing one)
just env-whoami     # prints your public recipient — send it to a maintainer
```

A maintainer adds your recipient to `.sops.yaml`, runs `just env-rekey`, and
commits. Until then you cannot decrypt anything. After that:

```sh
just env-decrypt    # env/enc/*.env.enc -> env/dec/*.env
just env-check      # confirms nothing plaintext is tracked and all files decrypt
```

## Day to day

| Command | What it does |
|---|---|
| `just env-list` | environments and the variable *names* in each (never values) |
| `just env-decrypt [name…]` | ciphertext → `env/dec/*.env`, mode 0600 |
| `just env-edit <name>` | open the decrypted file in `$EDITOR`; plaintext never hits disk |
| `just env-encrypt [name…]` | fold `env/dec/*.env` edits back into the ciphertext |
| `just env-status` | which variables differ between your `env/dec` and the ciphertext |
| `just env-run <name> <cmd…>` | run `cmd` with those variables exported, no plaintext on disk |
| `just env-new <name>` | start a new environment |
| `just env-rekey` | re-sync recipients after editing `.sops.yaml` |
| `just env-check` | fail-closed audit — safe to run in CI |
| `just env-clean` | wipe `env/dec` |

Prefer `just env-edit` over decrypt-edit-encrypt. Both work, but `env-edit`
re-encrypts only the values you actually changed, so the diff names them:

```
-DATABASE_URL=ENC[AES256_GCM,data:OG3trz…]
+DATABASE_URL=ENC[AES256_GCM,data:9fKq2a…]
```

`just env-encrypt` uses the same mechanism, so it is equally clean. A bare
`sops encrypt` is not — it gives every line a fresh IV and rewrites the whole
file, which makes review useless and guarantees merge conflicts. Don't call
sops directly; use the recipes.

## Running things

`.envrc` auto-loads **`env/dec/local.env` only** — non-production values for
your own machine. Staging and production are deliberately opt-in per command:

```sh
just env-run prod cargo run --release
just env-run staging ./scripts/migrate.sh
```

`env-run` streams the values straight into the child process. Nothing is
written to disk, so an interrupted run can't leave `env/dec/prod.env` behind.

## What is and isn't hidden

Variable **names are plaintext** in `env/enc/*.env.enc`; only values are
encrypted. That is the point — it makes diffs reviewable and lets `env-list`
work without a key. Never encode a secret in a variable *name*. Comments are
encrypted, so anything explanatory belongs in this file instead.

Two format limits, inherited from sops' dotenv parser:

- **No multi-line values.** A PEM must be a single line with `\n` escapes:
  `JWT_PRIVATE_KEY="-----BEGIN PRIVATE KEY-----\nMIIE…\n-----END PRIVATE KEY-----\n"`
- **Blank lines are dropped** on round-trip. Cosmetic only.

## Rules

- Never commit anything from `env/dec/`. `.gitignore` and `just env-check`
  both block it; don't defeat them with `git add -f`.
- Never commit a private age key. They belong only in
  `~/Library/Application Support/sops/age/keys.txt` (macOS) or
  `~/.config/sops/age/keys.txt` (Linux), mode 0600.
- Removing a recipient does not un-leak anything. Rotate the credentials too.
- Files ending in `.env` are gitignored repo-wide. If a repo has a legitimate
  non-secret `*.env` (for example generated cluster topology), allow it with an
  explicit `!` rule in `.gitignore` — deny by default, permit narrowly.

[sops]: https://github.com/getsops/sops
[age]: https://github.com/FiloSottile/age
