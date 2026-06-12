#!/usr/bin/env sh
# Shared reminder: k8s-cluster pins 15 git submodules to branches (see .gitmodules).
# Only nags when a submodule is uninitialized (-), out of date (+), or has a
# merge conflict (U) — so it's quiet when everything is already in sync.
st="$(git submodule status --recursive 2>/dev/null)" || exit 0
[ -z "$st" ] && exit 0
if printf '%s\n' "$st" | grep -qE '^[-+U]'; then
  printf '\n\033[1;33m↪ k8s-cluster: git submodules are out of sync.\033[0m\n'
  printf '  They are branch-pinned (master/main/dev). To sync:\n'
  printf '    \033[36mgit submodule sync --recursive && git submodule update --init --recursive --remote\033[0m\n\n'
fi
exit 0
