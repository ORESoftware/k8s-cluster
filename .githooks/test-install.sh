#!/usr/bin/env sh
set -eu

source_root=$(git rev-parse --show-toplevel)
tmp_root=$(mktemp -d "${TMPDIR:-/tmp}/k8s-hooks.XXXXXX")
trap 'rm -rf "$tmp_root"' EXIT HUP INT TERM

new_fixture() {
  name=$1
  dir="$tmp_root/$name"
  mkdir -p "$dir"
  git -C "$dir" init -q
  cp -R "$source_root/.githooks" "$dir/.githooks"
  printf '%s\n' "$dir"
}

plain=$(new_fixture plain)
(
  cd "$plain"
  ./.githooks/install.sh
)
[ "$(git -C "$plain" config --get core.hooksPath)" = .githooks ]
[ -x "$plain/.githooks/pre-commit" ]
[ -x "$plain/.githooks/pre-push" ]

custom=$(new_fixture custom)
git -C "$custom" config core.hooksPath company-hooks
(
  cd "$custom"
  ./.githooks/install.sh
)
[ "$(git -C "$custom" config --get core.hooksPath)" = company-hooks ] || {
  echo 'installer replaced custom core.hooksPath' >&2
  exit 1
}

printf '%s\n' '[git-hooks] PASS'
