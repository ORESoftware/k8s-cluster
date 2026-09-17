#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
pointer='Read and apply [`../agents.md`](../agents.md). This file is only a tool pointer.'

test -s "$repo_root/agents.md"
test -s "$repo_root/SECURITY.md"
for relative in .claude/CLAUDE.md .gemini/GEMINI.md .openai/AGENTS.md; do
  file="$repo_root/$relative"
  test -f "$file"
  test "$(wc -l < "$file" | tr -d ' ')" = 1
  test "$(sed -n '1p' "$file")" = "$pointer"
done

resolver="$repo_root/scripts/resolve-agent-instructions.py"
test -x "$resolver"

scratch=$(mktemp -d "$repo_root/.agent-check.XXXXXX")
cleanup() {
  case "$scratch" in
    "$repo_root"/.agent-check.*) rm -rf -- "$scratch" ;;
    *) echo "refusing to clean unexpected path: $scratch" >&2; return 1 ;;
  esac
}
trap cleanup EXIT

mkdir -p "$scratch/child/grandchild"
printf '# temporary parent instructions\n' > "$scratch/agents.md"
printf '# temporary child instructions\n' > "$scratch/child/agents.md"

actual=$(cd "$scratch/child/grandchild" && python3 "$resolver")
expected=$(printf '%s\n%s\n%s' \
  "$repo_root/agents.md" \
  "$scratch/agents.md" \
  "$scratch/child/agents.md")
test "$actual" = "$expected"

mkdir "$scratch/cycle"
ln -s agents.md "$scratch/cycle/agents.md"
if python3 "$resolver" "$scratch/cycle" >/dev/null 2>&1; then
  echo "resolver accepted a symlink cycle" >&2
  exit 1
fi

chmod 000 "$scratch/child/agents.md"
if test "$(id -u)" -ne 0 && python3 "$resolver" "$scratch/child" >/dev/null 2>&1; then
  echo "resolver accepted an unreadable agents.md" >&2
  exit 1
fi
chmod 600 "$scratch/child/agents.md"

echo "repository policy and root-to-leaf agent discovery are valid"
