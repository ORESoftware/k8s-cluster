#!/usr/bin/env bash
# Bump every apps/ submodule to its origin main HEAD and stage the new pins.
# Additive only: fetches and checks out a fetched commit, never rewrites history.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
git submodule foreach 'git fetch -q origin main && git checkout -q FETCH_HEAD'
git add apps
git status --short -- apps
echo "Pins staged; review and commit."
