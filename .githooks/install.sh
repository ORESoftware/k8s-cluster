#!/usr/bin/env bash
# Point this clone at the versioned hooks in `.githooks/`. Git does NOT auto-run
# a tracked hooks dir, so every clone (including the sync bot's) must run this
# once, or the reminders + the pre-push submodule guard simply don't fire.
#
#   ./.githooks/install.sh
#
# Idempotent. Safe to re-run.
set -eu
root="$(git rev-parse --show-toplevel)"
git -C "$root" config core.hooksPath .githooks
chmod +x "$root"/.githooks/pre-push \
         "$root"/.githooks/post-merge \
         "$root"/.githooks/post-checkout \
         "$root"/.githooks/post-rewrite \
         "$root"/.githooks/submodule-sync-reminder.sh \
         "$root"/.githooks/submodule-push-guard.sh 2>/dev/null || true
echo "hooks installed: core.hooksPath -> .githooks"
echo "  reminders: post-merge / post-checkout / post-rewrite"
echo "  guard:     pre-push (blocks unpushed submodule pointers)"
