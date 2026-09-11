#!/usr/bin/env bash
set -Eeuo pipefail

work=/tmp/den-2797-gated-e2e
rm -rf "$work"
mkdir -p "$work/sources" "$work/seeds" "$work/targets"

require_command() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "required command is missing: $1" >&2
    exit 127
  }
}

for command_name in git npm node python3 curl; do
  require_command "$command_name"
done

fetch_seed() {
  local repository="$1"
  local pull_number="$2"
  local expected_head="$3"
  local seed_path="$4"
  local destination="$5"
  local source_dir="$work/sources/${destination##*/}"

  git init --quiet "$source_dir"
  git -C "$source_dir" remote add origin "https://github.com/$repository.git"
  git -C "$source_dir" fetch --quiet --depth=1 origin "refs/pull/$pull_number/head"
  local actual_head
  actual_head="$(git -C "$source_dir" rev-parse FETCH_HEAD)"
  if [[ "$actual_head" != "$expected_head" ]]; then
    echo "pull ref moved for $repository#$pull_number: expected $expected_head, got $actual_head" >&2
    exit 31
  fi
  git -C "$source_dir" checkout --quiet --detach FETCH_HEAD
  test -d "$source_dir/$seed_path"
  mkdir -p "$destination"
  cp -a "$source_dir/$seed_path/." "$destination/"
  rm -rf "$destination/.git" "$destination/node_modules"
}

fetch_seed \
  apostille-me/.github \
  12 \
  "$APOSTILLE_SOURCE_SHA" \
  repository-seeds/apme-e2e \
  "$work/seeds/apme-e2e"

fetch_seed \
  embedded-alerts/.github \
  11 \
  "$EMBEDDED_SOURCE_SHA" \
  repository-seeds/eal-e2e \
  "$work/seeds/eal-e2e"

validate_seed() {
  local seed_dir="$1"
  test -s "$seed_dir/package.json"
  test -s "$seed_dir/.zpkg.toml"
  test -s "$seed_dir/publish.sh"
  bash -n "$seed_dir/publish.sh"

  (
    cd "$seed_dir"
    npm install --package-lock-only --ignore-scripts --package-lock=true
    test -s package-lock.json
    npm ci --ignore-scripts --package-lock=true
    npm run check
    rm -rf node_modules
  )
}

validate_seed "$work/seeds/apme-e2e"
validate_seed "$work/seeds/eal-e2e"

python3 - "$work/seeds/apme-e2e" "$work/seeds/eal-e2e" <<'PY'
from pathlib import Path
import re
import sys

prefixes = ("gh" + "p_", "lin_" + "api_", "cf" + "at_")
patterns = [re.compile(re.escape(prefix) + r"[A-Za-z0-9_-]{20,}") for prefix in prefixes]
for root_name in sys.argv[1:]:
    root = Path(root_name)
    for path in root.rglob("*"):
        if not path.is_file() or ".git" in path.parts:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for pattern in patterns:
            if pattern.search(text):
                raise SystemExit(f"credential-shaped value found in {path}")
PY

create_target() {
  local seed_dir="$1"
  local target_dir="$2"
  local source_repository="$3"
  local source_pull="$4"
  local source_sha="$5"
  local target_repository="$6"
  local timestamp="$7"

  mkdir -p "$target_dir"
  cp -a "$seed_dir/." "$target_dir/"
  rm -rf "$target_dir/.git" "$target_dir/node_modules"

  cat >"$target_dir/RECOVERY_PROVENANCE.md" <<EOF
# Recovery provenance

This repository was reconstructed from the canonical seed subtree in
\`$source_repository\` pull request #$source_pull at immutable source commit
\`$source_sha\`.

The source pull-request commit belongs to the organization policy repository,
not to this target repository's original Git history. This initial target
commit is therefore explicitly a reconstructed subtree publication; it does
not claim fabricated ancestry.

Publication is tracked by Linear DEN-2797 and used a fail-closed, no-force
workflow in \`ORESoftware/k8s-cluster\`.
EOF

  git init --quiet --initial-branch=main "$target_dir"
  git -C "$target_dir" config user.name "ORESoftware automation"
  git -C "$target_dir" config user.email "11139560+ORESoftware@users.noreply.github.com"
  git -C "$target_dir" add --all
  GIT_AUTHOR_DATE="$timestamp" GIT_COMMITTER_DATE="$timestamp" \
    git -C "$target_dir" commit --quiet -m "bootstrap canonical E2E harness" \
      -m "Reconstructed from $source_repository#$source_pull at $source_sha." \
      -m "Target: $target_repository. Tracking: DEN-2797."
  test -z "$(git -C "$target_dir" status --porcelain)"
}

create_target \
  "$work/seeds/apme-e2e" \
  "$work/targets/apme-e2e" \
  apostille-me/.github \
  12 \
  "$APOSTILLE_SOURCE_SHA" \
  apostille-me/apme-e2e \
  2026-08-09T20:31:00Z

create_target \
  "$work/seeds/eal-e2e" \
  "$work/targets/eal-e2e" \
  embedded-alerts/.github \
  11 \
  "$EMBEDDED_SOURCE_SHA" \
  embedded-alerts/eal-e2e \
  2026-08-09T20:32:00Z

APME_HEAD="$(git -C "$work/targets/apme-e2e" rev-parse HEAD)"
EAL_HEAD="$(git -C "$work/targets/eal-e2e" rev-parse HEAD)"
export APME_HEAD EAL_HEAD

python3 - "$work/manifest.json" <<'PY'
import json
import os
import sys

manifest = {
    "schema": 1,
    "targets": [
        {
            "repository": "apostille-me/apme-e2e",
            "directory": "/tmp/den-2797-gated-e2e/targets/apme-e2e",
            "head": os.environ["APME_HEAD"],
            "source_repository": "apostille-me/.github",
            "source_pull": 12,
            "source_sha": os.environ["APOSTILLE_SOURCE_SHA"],
            "history_disposition": "reconstructed-subtree",
        },
        {
            "repository": "embedded-alerts/eal-e2e",
            "directory": "/tmp/den-2797-gated-e2e/targets/eal-e2e",
            "head": os.environ["EAL_HEAD"],
            "source_repository": "embedded-alerts/.github",
            "source_pull": 11,
            "source_sha": os.environ["EMBEDDED_SOURCE_SHA"],
            "history_disposition": "reconstructed-subtree",
        },
    ],
}
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump(manifest, handle, indent=2, sort_keys=True)
    handle.write("\n")
PY

api_base="${GITHUB_API_URL:-https://api.github.com}"
for repository in apostille-me/apme-e2e embedded-alerts/eal-e2e; do
  status="$(curl --silent --show-error --output "$work/repository-check.json" --write-out '%{http_code}' \
    -H 'Accept: application/vnd.github+json' \
    -H 'X-GitHub-Api-Version: 2022-11-28' \
    "$api_base/repos/$repository")"
  case "$status" in
    404) ;;
    200)
      echo "$repository already exists before credential handoff; refusing the publication race" >&2
      exit 32
      ;;
    *)
      echo "unexpected repository preflight response for $repository: HTTP $status" >&2
      exit 33
      ;;
  esac
done

printf 'prepared %s at %s\n' apostille-me/apme-e2e "$APME_HEAD"
printf 'prepared %s at %s\n' embedded-alerts/eal-e2e "$EAL_HEAD"
