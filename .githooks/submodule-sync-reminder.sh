#!/usr/bin/env bash
# Print a reminder when this repo's git submodules are out of sync with the
# gitlinks recorded in the index — i.e. after a pull/merge/checkout/rebase moved
# a submodule pointer but the submodule working tree wasn't updated to match.
#
# `git submodule status` marks each out-of-sync submodule with a leading flag:
#   '+'  checked-out commit differs from the one recorded in the superproject
#   '-'  submodule is not initialized
#   'U'  submodule has merge conflicts
# A leading space means "in sync" (no reminder needed for that one).
#
# This only PRINTS a reminder; it never mutates anything (a hook that silently
# ran `submodule update` could clobber intentional in-progress submodule work).
#
# Sourced/called by the post-merge, post-checkout, and post-rewrite hooks.

set -u

# Respect an opt-out for scripted/non-interactive contexts (CI, the sync bot).
case "${DD_SKIP_SUBMODULE_REMINDER:-}" in
    1 | true | yes | on) exit 0 ;;
esac

repo_root="$(git rev-parse --show-toplevel 2>/dev/null)" || exit 0
[ -f "$repo_root/.gitmodules" ] || exit 0

# Collect out-of-sync submodule paths (lines NOT starting with a space).
out_of_sync="$(git -C "$repo_root" submodule status 2>/dev/null \
    | grep -E '^[-+U]' || true)"
[ -n "$out_of_sync" ] || exit 0

count="$(printf '%s\n' "$out_of_sync" | grep -c .)"

# ANSI bold/yellow only when stderr is a TTY.
if [ -t 2 ]; then b='\033[1m'; y='\033[33m'; r='\033[0m'; else b=''; y=''; r=''; fi

{
    printf '%b' "${y}"
    printf '┌─────────────────────────────────────────────────────────────────────┐\n'
    printf '│ ' ; printf "${b}⚠  %s git submodule(s) are out of sync with this checkout${r}${y}" "$count" ; printf '\n'
    printf '├─────────────────────────────────────────────────────────────────────┤\n'
    printf '%b' "${r}"
    printf '%s\n' "$out_of_sync" | while IFS= read -r line; do
        flag="${line:0:1}"
        path="$(printf '%s\n' "$line" | awk '{print $2}')"
        case "$flag" in
            '+') note='pointer moved — working tree behind/ahead of the recorded commit' ;;
            '-') note='not initialized' ;;
            'U') note='merge conflict' ;;
            *)   note='out of sync' ;;
        esac
        printf '    %s  (%s)\n' "$path" "$note"
    done
    printf '%b' "${y}"
    printf '├─────────────────────────────────────────────────────────────────────┤\n'
    printf '│ Sync them with:                                                     │\n'
    printf '%b' "${r}"
    printf '    git submodule update --init --recursive\n'
    printf '%b' "${y}"
    printf '│ (set DD_SKIP_SUBMODULE_REMINDER=1 to silence this reminder)          │\n'
    printf '└─────────────────────────────────────────────────────────────────────┘\n'
    printf '%b' "${r}"
} >&2

exit 0
