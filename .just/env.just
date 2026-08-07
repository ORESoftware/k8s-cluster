# ─── fiducia-cloud · encrypted environment files ────────────────────────────
#
# SHARED MODULE — byte-identical in every fiducia-cloud repo. Fix it in one
# repo and propagate; do not fork it per repo.
#
#   env/enc/<name>.env.enc   ciphertext — COMMITTED to git
#   env/dec/<name>.env       plaintext  — GITIGNORED, mode 0600
#
# The files are dotenv under the hood, so variable NAMES stay readable in the
# ciphertext and a diff shows exactly which values changed. `.enc` is not a
# format sops auto-detects, so every call states the types explicitly.
#
#   just env-keygen                  once per machine
#   just env-whoami                  your recipient — add it to .sops.yaml
#   just env-decrypt                 enc -> dec, for tools needing a real file
#   just env-edit local              change a value (minimal diff)
#   just env-run prod cargo run      no plaintext ever touches disk
#   just env-check                   fail-closed audit — wire into CI

# So `env-run` receives its command as real argv ($1, $2, …) instead of one
# flattened string — otherwise `just env-run local sh -c 'a b'` loses the quotes.
set positional-arguments := true

_enc := justfile_directory() / "env" / "enc"
_dec := justfile_directory() / "env" / "dec"
_fmt := "--input-type dotenv --output-type dotenv"

# List every environment and the variable names it defines (never values).
[group('env')]
env-list:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    files=({{ _enc }}/*.env.enc)
    if [[ ${#files[@]} -eq 0 ]]; then echo "no environments in env/enc/"; exit 0; fi
    for f in "${files[@]}"; do
      name=$(basename "$f" .env.enc)
      # Drop sops' own trailing metadata rows (sops_mac, sops_version, …).
      keys=$(grep -oE '^[A-Za-z_][A-Za-z0-9_]*=' "$f" | tr -d '=' \
             | grep -v '^sops_' | tr '\n' ' ')
      printf '%-12s %s\n' "$name" "$keys"
    done

# Decrypt env/enc/*.env.enc into env/dec/*.env (mode 0600). No args = all.
[group('env')]
env-decrypt *names:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    mkdir -p {{ _dec }} && chmod 700 {{ _dec }}
    sel=({{ names }})
    if [[ ${#sel[@]} -eq 0 ]]; then
      for f in {{ _enc }}/*.env.enc; do sel+=("$(basename "$f" .env.enc)"); done
    fi
    if [[ ${#sel[@]} -eq 0 ]]; then echo "nothing to decrypt"; exit 0; fi
    for n in "${sel[@]}"; do
      src={{ _enc }}/"$n".env.enc
      [[ -f $src ]] || { echo "no such environment: $n" >&2; exit 1; }
      out={{ _dec }}/"$n".env
      # Write via a 0600 temp file so plaintext is never briefly world-readable.
      tmp=$(mktemp "{{ _dec }}/.$n.XXXXXX")
      chmod 600 "$tmp"
      sops decrypt {{ _fmt }} "$src" > "$tmp"
      mv -f "$tmp" "$out"
      echo "decrypted  env/dec/$n.env"
    done

# Encrypt env/dec/<name>.env back into env/enc/<name>.env.enc. No args = all.
[group('env')]
env-encrypt *names:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    sel=({{ names }})
    if [[ ${#sel[@]} -eq 0 ]]; then
      for f in {{ _dec }}/*.env; do sel+=("$(basename "$f" .env)"); done
    fi
    if [[ ${#sel[@]} -eq 0 ]]; then echo "nothing in env/dec to encrypt"; exit 0; fi
    mkdir -p {{ _enc }}
    for n in "${sel[@]}"; do
      src={{ _dec }}/"$n".env
      [[ -f $src ]] || { echo "no such plaintext: env/dec/$n.env" >&2; exit 1; }
      dst={{ _enc }}/"$n".env.enc
      if [[ -f $dst ]]; then
        # Drive `sops edit` with a copy-as-editor so sops re-encrypts ONLY the
        # values that actually changed. A plain `sops encrypt` would give every
        # line a fresh IV and churn the whole file on every run.
        # sops runs $EDITOR through `sh -c`, so the path needs shell quoting.
        rc=0
        err=$(EDITOR="cp -f -- $(printf '%q' "$src")" sops edit {{ _fmt }} "$dst" 2>&1) || rc=$?
        # 200 is sops' "file has not changed" — a no-op here, not a failure.
        case $rc in
          0)   echo "encrypted  env/enc/$n.env.enc" ;;
          200) echo "unchanged  env/enc/$n.env.enc" ;;
          *)   printf '%s\n' "$err" >&2; exit "$rc" ;;
        esac
      else
        sops encrypt {{ _fmt }} --filename-override "$dst" "$src" > "$dst"
        echo "encrypted  env/enc/$n.env.enc"
      fi
    done

# Edit an encrypted environment in $EDITOR; plaintext never lands on disk.
[group('env')]
env-edit name:
    @sops edit {{ _fmt }} {{ _enc }}/{{ name }}.env.enc

# Create a new empty encrypted environment.
[group('env')]
env-new name:
    #!/usr/bin/env bash
    set -euo pipefail
    dst={{ _enc }}/{{ name }}.env.enc
    [[ -e $dst ]] && { echo "already exists: env/enc/{{ name }}.env.enc" >&2; exit 1; }
    mkdir -p {{ _enc }}
    printf '# %s environment for this repo. Values are encrypted; names are not.\n' {{ name }} \
      | sops encrypt {{ _fmt }} --filename-override "$dst" /dev/stdin > "$dst"
    echo "created  env/enc/{{ name }}.env.enc — populate it with: just env-edit {{ name }}"

# Run a command with <name>'s variables exported. Plaintext never hits disk.
[group('env')]
env-run name +cmd:
    #!/usr/bin/env bash
    set -euo pipefail
    src={{ _enc }}/{{ name }}.env.enc
    [[ -f $src ]] || { echo "no such environment: {{ name }}" >&2; exit 1; }
    # Deliberately NOT `sops exec-env`: it treats the raw dotenv value as
    # opaque, so DQ="hello world" is exported with the quote characters still
    # attached, while every app-side dotenv library strips them. Parse it here
    # with the ordinary dotenv rules instead, so `just env-run` and an app
    # reading env/dec/<name>.env see byte-identical values.
    #   "…"  quotes stripped, \n \t \r \\ expanded
    #   '…'  quotes stripped, contents literal
    #   …    taken literally, spaces and all
    # The plaintext only ever exists in this process's memory and the child's
    # environment — nothing is written to disk.
    eval "$(sops decrypt {{ _fmt }} "$src" | while IFS= read -r line || [[ -n $line ]]; do
      [[ $line =~ ^[[:space:]]*(#|$) ]] && continue
      line=${line#export }
      [[ $line =~ ^([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]] || continue
      k=${BASH_REMATCH[1]}; v=${BASH_REMATCH[2]}
      [[ $k == sops_* ]] && continue
      if [[ ${#v} -ge 2 && $v == \"*\" ]]; then v=$(printf '%b' "${v:1:${#v}-2}")
      elif [[ ${#v} -ge 2 && $v == \'*\' ]]; then v=${v:1:${#v}-2}
      fi
      printf 'export %s=%q\n' "$k" "$v"
    done)"
    shift          # drop the environment name; "$@" is now the command
    exec "$@"

# Show which variables differ between env/dec/*.env and the committed ciphertext.
[group('env')]
env-status:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    clean=1
    for f in {{ _enc }}/*.env.enc; do
      n=$(basename "$f" .env.enc); p={{ _dec }}/"$n".env
      if [[ ! -f $p ]]; then printf '%-12s not decrypted locally\n' "$n"; continue; fi
      if diff -q <(sops decrypt {{ _fmt }} "$f") "$p" >/dev/null 2>&1; then
        printf '%-12s in sync\n' "$n"
      else
        clean=0
        # `diff` exits 1 here by definition, and grep exits 1 on no match;
        # neither is an error for us, so keep pipefail from aborting the recipe.
        changed=$( { diff <(sops decrypt {{ _fmt }} "$f") "$p" || true; } \
          | { grep -oE '^[<>] [A-Za-z_][A-Za-z0-9_]*=' || true; } \
          | sed 's/^..//;s/=$//' | sort -u | tr '\n' ' ')
        printf '%-12s MODIFIED locally: %s\n' "$n" "${changed:-<comments/order only>}"
      fi
    done
    [[ $clean -eq 1 ]] || echo $'\nrun `just env-encrypt` to fold those changes into the ciphertext'

# Re-encrypt to match .sops.yaml after adding or removing a recipient.
[group('env')]
env-rekey:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    for f in {{ _enc }}/*.env.enc; do
      sops updatekeys --yes --input-type dotenv "$f"
    done
    echo "recipients synced — commit the result"

# Print this machine's age recipient (public key; safe to share and commit).
[group('env')]
env-whoami:
    #!/usr/bin/env bash
    set -euo pipefail
    for p in "${SOPS_AGE_KEY_FILE:-}" \
             "${XDG_CONFIG_HOME:-$HOME/.config}/sops/age/keys.txt" \
             "$HOME/Library/Application Support/sops/age/keys.txt"; do
      [[ -n $p && -f $p ]] && { age-keygen -y "$p"; exit 0; }
    done
    echo "no age key found — run: just env-keygen" >&2; exit 1

# Create this machine's age key, once. Never overwrites an existing key.
[group('env')]
env-keygen:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ $OSTYPE == darwin* ]]; then
      dir="$HOME/Library/Application Support/sops/age"
    else
      dir="${XDG_CONFIG_HOME:-$HOME/.config}/sops/age"
    fi
    key="$dir/keys.txt"
    if [[ -f $key ]]; then
      echo "key already exists at $key"; age-keygen -y "$key"; exit 0
    fi
    mkdir -p "$dir" && chmod 700 "$dir"
    age-keygen -o "$key" 2>/dev/null
    chmod 600 "$key"
    echo "created $key"
    echo "add this recipient to .sops.yaml, then ask someone to run \`just env-rekey\`:"
    age-keygen -y "$key"

# Fail-closed audit: no plaintext in git, gitignore intact, ciphertext valid.
[group('env')]
env-check:
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{ justfile_directory() }}
    fail=0
    note() { echo "  ✗ $1"; fail=1; }

    echo "tracked files:"
    # Dotenv-shaped secrets: `.env`, `.env.<anything>`, and anything under
    # env/dec/. A file like clusters/x/topology.env is non-secret generated
    # config, not a dotenv secret — it is allowed via a `!` rule in .gitignore
    # and is covered by the force-add check below instead.
    bad=$(git ls-files -z 2>/dev/null | tr '\0' '\n' \
      | grep -E '(^|/)\.env$|(^|/)\.env\.[^/]*$|(^|/)env/dec/' \
      | grep -vE '\.(example|sample|template)$' || true)
    if [[ -n $bad ]]; then note "plaintext env files are tracked:"; echo "$bad" | sed 's/^/      /'
    else echo "  ✓ no dotenv secrets tracked"; fi

    # A tracked file that .gitignore also matches was force-added past the
    # rules — the signature of a leaked secret. Scoped to secret-shaped paths on
    # purpose: repos legitimately track a Cargo.lock or an asset that some other
    # rule ignores, and failing on those would just train people to ignore this
    # check. Negated paths (topology.env) aren't ignored, so they never appear.
    # --no-index is required: without it check-ignore skips tracked files,
    # which are precisely the ones we are looking for here.
    forced=$(git ls-files -z 2>/dev/null | tr '\0' '\n' \
      | git check-ignore --no-index --stdin 2>/dev/null \
      | grep -E '(^|/)env/|\.env$|\.age-?key$|(^|/)keys\.txt$|sops-private' || true)
    if [[ -n $forced ]]; then note "tracked despite .gitignore (force-added?):"; echo "$forced" | sed 's/^/      /'
    else echo "  ✓ nothing force-added past .gitignore"; fi

    echo "staged changes:"
    staged=$(git diff --cached --name-only -z 2>/dev/null | tr '\0' '\n' \
      | grep -E '(^|/)\.env$|(^|/)\.env\.[^/]*$|(^|/)env/dec/' \
      | grep -vE '\.(example|sample|template)$' || true)
    if [[ -n $staged ]]; then note "plaintext env files are staged:"; echo "$staged" | sed 's/^/      /'
    else echo "  ✓ nothing plaintext staged"; fi

    echo "private key material:"
    keys=$(git ls-files -z 2>/dev/null | tr '\0' '\n' \
      | grep -E '\.(agekey|age-key)$|(^|/)keys\.txt$|AGE-SECRET-KEY' || true)
    if [[ -n $keys ]]; then note "possible private keys tracked:"; echo "$keys" | sed 's/^/      /'
    else echo "  ✓ no age private keys tracked"; fi

    echo ".gitignore:"
    for rule in '.env' '*.env' '**/*.env' 'env/dec/'; do
      grep -qxF "$rule" .gitignore 2>/dev/null \
        && echo "  ✓ $rule" || note "missing .gitignore rule: $rule"
    done

    echo "ciphertext integrity:"
    shopt -s nullglob
    found=0
    for f in env/enc/*.env.enc; do
      found=1
      if sops decrypt --input-type dotenv --output-type dotenv "$f" >/dev/null 2>&1; then
        echo "  ✓ $f decrypts"
      else
        note "$f does NOT decrypt with your key (ask for a rekey)"
      fi
    done
    [[ $found -eq 1 ]] || echo "  · no environments yet"

    [[ $fail -eq 0 ]] && echo $'\nenv-check PASSED' || { echo $'\nenv-check FAILED'; exit 1; }

# Wipe decrypted plaintext from env/dec (regenerate with `just env-decrypt`).
[group('env')]
env-clean:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    n=0
    for f in {{ _dec }}/*.env; do
      # Overwrite before unlinking so the plaintext does not linger in free blocks.
      dd if=/dev/zero of="$f" bs=1 count="$(wc -c <"$f")" conv=notrunc status=none 2>/dev/null || true
      rm -f -- "$f"
      n=$((n+1))
    done
    echo "removed $n decrypted file(s) from env/dec/"
