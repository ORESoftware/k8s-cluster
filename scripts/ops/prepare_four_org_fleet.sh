#!/usr/bin/env bash
set -Eeuo pipefail

on_error() {
  local status=$?
  local line=${BASH_LINENO[0]:-${LINENO}}
  local command=${BASH_COMMAND:-unknown}
  trap - ERR
  printf 'FLEET_PREP_ERROR status=%s line=%s command=%q\n' "$status" "$line" "$command" >&2
  exit "$status"
}
trap on_error ERR

fail() {
  printf 'FLEET_PREP_ERROR %s\n' "$*" >&2
  exit 1
}

FLEET_ROOT="${1:?usage: prepare-four-org-fleet.sh FLEET_ROOT OVERLAY_ROOT}"
OVERLAY_ROOT="${2:?usage: prepare-four-org-fleet.sh FLEET_ROOT OVERLAY_ROOT}"
[[ -d "$FLEET_ROOT" ]] || fail "fleet root does not exist: $FLEET_ROOT"
[[ -d "$OVERLAY_ROOT" ]] || fail "overlay root does not exist: $OVERLAY_ROOT"
[[ -f "$OVERLAY_ROOT/publish-all.sh" ]] || fail "overlay publisher is missing: $OVERLAY_ROOT/publish-all.sh"

export GIT_AUTHOR_NAME="${GIT_AUTHOR_NAME:-ChatGPT Codex}"
export GIT_AUTHOR_EMAIL="${GIT_AUTHOR_EMAIL:-41898282+github-actions[bot]@users.noreply.github.com}"
export GIT_COMMITTER_NAME="$GIT_AUTHOR_NAME"
export GIT_COMMITTER_EMAIL="$GIT_AUTHOR_EMAIL"

organizations=(apostille-me evento-globolo hacker-house-medellin embedded-alerts)

prefix_for() {
  case "$1" in
    apostille-me) echo apme ;;
    evento-globolo) echo evgl ;;
    hacker-house-medellin) echo hhm ;;
    embedded-alerts) echo eal ;;
    *) return 1 ;;
  esac
}

existing_repositories_for() {
  case "$1" in
    apostille-me) printf '%s\n' apme-api apme-cli apme-infra apme-interfaces apme-sync apme-web-dioxus apme-web-leptos apme-web-mash ;;
    evento-globolo) printf '%s\n' evgl-api evgl-cli evgl-infra evgl-interfaces evgl-sync evgl-dioxus-web evgl-leptos-web evgl-mash-web ;;
    hacker-house-medellin) printf '%s\n' hhm-api hhm-cli hhm-infra hhm-interfaces hhm-sync hhm-dioxus-web hhm-leptos-web hhm-mash-web ;;
    embedded-alerts) printf '%s\n' eal-api eal-cli eal-infra eal-interfaces eal-sync eal-dioxus-web eal-leptos-web eal-mash-web ;;
  esac
}

normalize_existing_repository() {
  local org=$1 repo=$2
  local path="$FLEET_ROOT/$org/$repo"
  [[ -d "$path" ]] || fail "expected generated repository directory is missing: $org/$repo"
  [[ -d "$path/.git" ]] || fail "expected generated Git repository is missing: $org/$repo"
  git -C "$path" config user.name "$GIT_AUTHOR_NAME"
  git -C "$path" config user.email "$GIT_AUTHOR_EMAIL"
  git -C "$path" show-ref --verify --quiet refs/heads/main \
    || fail "generated repository has no main branch: $org/$repo"
  git -C "$path" checkout -q main
  git -C "$path" rev-parse --verify HEAD >/dev/null \
    || fail "generated repository has no main commit: $org/$repo"

  local dirty
  dirty="$(git -C "$path" status --porcelain)"
  if [[ -n "$dirty" ]]; then
    printf 'Normalizing generated files in %s/%s:\n%s\n' "$org" "$repo" "$dirty"
    git -C "$path" add -A
    git -C "$path" commit -m 'chore: normalize generated repository'
  fi
  [[ -z "$(git -C "$path" status --porcelain)" ]] \
    || fail "generated repository remains dirty after normalization: $org/$repo"
}

printf 'FLEET_PREP_STAGE normalize-existing repositories=32\n'
for org in "${organizations[@]}"; do
  while IFS= read -r repo; do
    normalize_existing_repository "$org" "$repo"
  done < <(existing_repositories_for "$org")
done

prepare_new_repository() {
  local org=$1 repo=$2 label=$3
  local source="$OVERLAY_ROOT/$org/$repo"
  local destination="$FLEET_ROOT/$org/$repo"
  [[ -d "$source" ]] || fail "overlay source is missing: $org/$repo"
  [[ ! -e "$destination" ]] || fail "new repository destination already exists: $org/$repo"
  mkdir -p "$destination"
  git -C "$destination" init --initial-branch=main
  cat > "$destination/README.md" <<README
# $repo

Repository initialized for **$org**. The complete $label implementation is proposed through the bootstrap feature branch.
README
  cat > "$destination/.gitignore" <<'IGNORE'
.DS_Store
node_modules/
dist/
target/
.env
IGNORE
  git -C "$destination" add -A
  git -C "$destination" commit -m 'chore: initialize repository'
  git -C "$destination" checkout -q -b agent/bootstrap-repository-family
  find "$destination" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
  cp -a "$source/." "$destination/"
  git -C "$destination" add -A
  git -C "$destination" commit -m "feat: bootstrap $label"
  git -C "$destination" checkout -q main
  git -C "$destination" fsck --no-reflogs --full >/dev/null
  test -z "$(git -C "$destination" status --porcelain)"
}

printf 'FLEET_PREP_STAGE create-new repositories=16\n'
for org in "${organizations[@]}"; do
  prefix="$(prefix_for "$org")"
  prepare_new_repository "$org" "${prefix}-clients" 'typed clients and SDKs'
  prepare_new_repository "$org" "${prefix}-libs" 'shared libraries'
  prepare_new_repository "$org" "${prefix}-monorepo" 'integrated monorepo'
  prepare_new_repository "$org" "${org}.github.io" 'Astro marketing site'
done

prepare_worker_branch() {
  local org=$1 repo=$2
  local path="$FLEET_ROOT/$org/$repo"
  local worker_source="$OVERLAY_ROOT/$org/$repo/cloudflare-worker"
  [[ -d "$worker_source" ]] || fail "Cloudflare Worker overlay is missing: $org/$repo"
  git -C "$path" checkout -q main
  if git -C "$path" show-ref --verify --quiet refs/heads/agent/add-cloudflare-worker-edge; then
    echo "unexpected existing Worker feature branch in $org/$repo" >&2
    return 1
  fi
  [[ ! -e "$path/cloudflare-worker" ]] || fail "Cloudflare Worker already exists on generated main: $org/$repo"
  git -C "$path" checkout -q -b agent/add-cloudflare-worker-edge
  cp -a "$worker_source" "$path/cloudflare-worker"
  mkdir -p "$path/.github/workflows"
  cat > "$path/.github/workflows/cloudflare-worker.yml" <<'YAML'
name: Cloudflare Worker

on:
  pull_request:
    paths:
      - cloudflare-worker/**
      - .github/workflows/cloudflare-worker.yml
  push:
    branches: [main]
    paths:
      - cloudflare-worker/**
      - .github/workflows/cloudflare-worker.yml

permissions:
  contents: read

jobs:
  verify:
    runs-on: ubuntu-24.04
    defaults:
      run:
        working-directory: cloudflare-worker
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '22'
      - run: npm install --no-audit --no-fund --package-lock=false
      - run: npm run lint
      - run: npm test
      - run: npm run deploy:dry
YAML
  if ! grep -qxF 'cloudflare-worker/node_modules/' "$path/.gitignore" 2>/dev/null; then
    printf '\ncloudflare-worker/node_modules/\ncloudflare-worker/.wrangler/\n' >> "$path/.gitignore"
  fi
  if [[ -f "$path/Makefile" ]] && ! grep -q '^worker-test:' "$path/Makefile"; then
    cat >> "$path/Makefile" <<'MAKE'

.PHONY: worker-test worker-dry-run
worker-test:
	cd cloudflare-worker && npm test

worker-dry-run:
	cd cloudflare-worker && npm run deploy:dry
MAKE
  fi
  cat >> "$path/README.md" <<'README'

## Cloudflare Worker edge gateway

The `cloudflare-worker/` package provides a Wrangler-managed edge gateway with health checks, signed webhook intake, validation, queue fan-out, security headers, unit tests, and a dry-run deployment command. The Worker is intentionally isolated from cluster infrastructure so it can be reviewed and deployed independently.
README
  git -C "$path" add -A
  git -C "$path" commit -m 'feat: add Cloudflare Worker edge gateway'
  git -C "$path" checkout -q main
  git -C "$path" fsck --no-reflogs --full >/dev/null
  test -z "$(git -C "$path" status --porcelain)"
}

printf 'FLEET_PREP_STAGE create-worker-branches repositories=4\n'
for org in "${organizations[@]}"; do
  prefix="$(prefix_for "$org")"
  prepare_worker_branch "$org" "${prefix}-infra"
done

mkdir -p "$FLEET_ROOT/scripts"
publisher="$FLEET_ROOT/scripts/publish-all.sh"
install -m 0755 "$OVERLAY_ROOT/publish-all.sh" "$publisher"
sed -i \
  -e 's/hhm-web-dioxus/hhm-dioxus-web/g' \
  -e 's/hhm-web-leptos/hhm-leptos-web/g' \
  -e 's/hhm-web-mash/hhm-mash-web/g' \
  -e 's/eal-web-dioxus/eal-dioxus-web/g' \
  -e 's/eal-web-leptos/eal-leptos-web/g' \
  -e 's/eal-web-mash/eal-mash-web/g' \
  "$publisher"
if grep -Eq '(hhm|eal)-web-(dioxus|leptos|mash)' "$publisher"; then
  fail 'publisher still contains legacy Hacker House or Embedded Alerts web-repository names'
fi

repo_count=0
feature_count=0
for org in "${organizations[@]}"; do
  for path in "$FLEET_ROOT/$org"/*; do
    [[ -d "$path/.git" ]] || continue
    repo_count=$((repo_count + 1))
    git -C "$path" show-ref --verify --quiet refs/heads/main
    test -z "$(git -C "$path" status --porcelain)"
    if git -C "$path" show-ref --verify --quiet refs/heads/agent/bootstrap-repository-family; then
      feature_count=$((feature_count + 1))
    fi
    if git -C "$path" show-ref --verify --quiet refs/heads/agent/add-cloudflare-worker-edge; then
      feature_count=$((feature_count + 1))
    fi
  done
done
[[ "$repo_count" -eq 48 ]] || fail "expected 48 repositories, found $repo_count"
[[ "$feature_count" -eq 20 ]] || fail "expected 20 review branches, found $feature_count"

printf 'FLEET_PREP_STAGE run-tests repositories=20\n'
for org in "${organizations[@]}"; do
  prefix="$(prefix_for "$org")"
  for repo in "${prefix}-clients" "${prefix}-libs" "${prefix}-monorepo"; do
    path="$FLEET_ROOT/$org/$repo"
    git -C "$path" checkout -q agent/bootstrap-repository-family
    (cd "$path" && npm test && npm run lint)
    git -C "$path" checkout -q main
  done

  marketing="$FLEET_ROOT/$org/${org}.github.io"
  git -C "$marketing" checkout -q agent/bootstrap-repository-family
  test -f "$marketing/astro.config.mjs"
  test -f "$marketing/src/pages/index.astro"
  test ! -e "$marketing/_config.yml"
  if [[ "${SKIP_NETWORK_INSTALLS:-0}" == 1 ]]; then
    (cd "$marketing" && npm test)
  else
    (cd "$marketing" && npm install --no-audit --no-fund --package-lock=false && npm test && npm run build)
  fi
  rm -rf "$marketing/node_modules" "$marketing/dist" "$marketing/.astro"
  git -C "$marketing" checkout -q main

  infra="$FLEET_ROOT/$org/${prefix}-infra"
  git -C "$infra" checkout -q agent/add-cloudflare-worker-edge
  (
    cd "$infra/cloudflare-worker"
    if [[ "${SKIP_NETWORK_INSTALLS:-0}" != 1 ]]; then
      npm install --no-audit --no-fund --package-lock=false
    fi
    npm run lint
    npm test
    if [[ "${SKIP_NETWORK_INSTALLS:-0}" != 1 ]]; then
      npm run deploy:dry
    fi
  )
  rm -rf "$infra/cloudflare-worker/node_modules" "$infra/cloudflare-worker/.wrangler"
  git -C "$infra" checkout -q main

done

for org in "${organizations[@]}"; do
  for path in "$FLEET_ROOT/$org"/*; do
    [[ -d "$path/.git" ]] || continue
    test -z "$(git -C "$path" status --porcelain)"
  done
done

printf 'FLEET_PREPARED repositories=%s review_branches=%s astro_sites=4 worker_packages=4\n' "$repo_count" "$feature_count"
