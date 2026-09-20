#!/usr/bin/env bash
# Activate this repository's versioned hooks when the clone has not already
# delegated hook ownership somewhere else. Git does NOT auto-run a tracked
# hooks dir, so every clone (including the sync bot's) must run this once, or
# the reminders + guards simply don't fire.
#
#   ./.githooks/install.sh
#
# Idempotent. Safe to re-run. An existing custom core.hooksPath is preserved.
set -eu
root="$(git rev-parse --show-toplevel)"

chmod +x "$root"/.githooks/pre-commit \
         "$root"/.githooks/pre-push \
         "$root"/.githooks/post-merge \
         "$root"/.githooks/post-checkout \
         "$root"/.githooks/post-rewrite \
         "$root"/.githooks/submodule-sync-reminder.sh \
         "$root"/.githooks/submodule-push-guard.sh 2>/dev/null || true

hooks_path="$(git -C "$root" config --get core.hooksPath 2>/dev/null || true)"
case "$hooks_path" in
  '')
    git -C "$root" config core.hooksPath .githooks
    echo "hooks installed: core.hooksPath -> .githooks"
    ;;
  .githooks)
    echo "hooks already active: core.hooksPath -> .githooks"
    ;;
  *)
    echo "hooks not activated: preserving existing core.hooksPath=$hooks_path" >&2
    echo "  merge/chaining must be handled explicitly before switching hook ownership" >&2
    exit 0
    ;;
esac

echo "  secret guard: pre-commit (gitleaks when installed)"
echo "  reminders:    post-merge / post-checkout / post-rewrite"
echo "  push guard:   pre-push (blocks unpushed submodule pointers)"
