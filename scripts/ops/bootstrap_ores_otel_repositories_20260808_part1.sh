#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly BOOTSTRAP_ID='ores-otel-2026-08-08-v1'
readonly SOURCE_REPOSITORY='ORESoftware/next-loggers.ts'
readonly CANONICAL_REPOSITORY='ores-otel/ores.otel.log'
readonly TEST_ORGANIZATION='ores-otel-test'
readonly API_VERSION='2022-11-28'

: "${GH_TOKEN:?GH_TOKEN is required}"

work="$(mktemp -d "${RUNNER_TEMP:-/tmp}/ores-otel-bootstrap.XXXXXX")"
cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  rm -rf "$work"
}
trap cleanup EXIT
trap 'rc=$?; printf "bootstrap-stage=%s status=failed rc=%s\n" "${stage:-initialization}" "$rc" >&2; exit "$rc"' ERR

stage=credential-transport
askpass="$work/git-askpass.sh"
cat > "$askpass" <<'ASKPASS'
#!/usr/bin/env sh
case "${1:-}" in
  *Username*) printf '%s\n' 'x-access-token' ;;
  *Password*) printf '%s\n' "${GH_TOKEN:?GH_TOKEN is required}" ;;
  *) exit 1 ;;
esac
ASKPASS
chmod 700 "$askpass"
export GIT_ASKPASS="$askpass"
export GIT_ASKPASS_REQUIRE=force
export GIT_TERMINAL_PROMPT=0
printf 'bootstrap-stage=%s status=passed\n' "$stage"

stage=publisher-preflight
publisher_login="$(gh api --header "X-GitHub-Api-Version: $API_VERSION" user --jq .login)"
test "$publisher_login" = 'ORESoftware'
for organization in ores-otel ores-otel-test; do
  membership="$(gh api --header "X-GitHub-Api-Version: $API_VERSION" \
    "user/memberships/orgs/$organization" --jq '.role + ":" + .state')"
  test "$membership" = 'admin:active'
  printf 'VERIFIED_OWNER %s\n' "$organization"
done
printf 'bootstrap-stage=%s status=passed publisher=%s\n' "$stage" "$publisher_login"

ensure_public_repository() {
  local owner="$1"
  local name="$2"
  local description="$3"
  local full_name="$owner/$name"
  local actual visibility

  if actual="$(gh api --header "X-GitHub-Api-Version: $API_VERSION" \
      "repos/$full_name" --jq .full_name 2>/dev/null)"; then
    test "${actual,,}" = "${full_name,,}"
    visibility="$(gh api --header "X-GitHub-Api-Version: $API_VERSION" \
      "repos/$full_name" --jq .visibility)"
    test "$visibility" = 'public'
    printf 'PRESERVE_REPOSITORY %s visibility=%s\n' "$full_name" "$visibility"
    return
  fi

  gh api --method POST --header "X-GitHub-Api-Version: $API_VERSION" \
    "orgs/$owner/repos" \
    -f name="$name" \
    -f description="$description" \
    -F private=false \
    -F has_issues=true \
    -F has_projects=false \
    -F has_wiki=false \
    -F auto_init=false \
    --silent
  actual="$(gh api --header "X-GitHub-Api-Version: $API_VERSION" \
    "repos/$full_name" --jq .full_name)"
  test "${actual,,}" = "${full_name,,}"
  printf 'CREATED_REPOSITORY %s visibility=public\n' "$full_name"
}

remote_has_main() {
  local full_name="$1"
  git ls-remote --exit-code "https://github.com/$full_name.git" refs/heads/main >/dev/null 2>&1
}

set_topics() {
  local full_name="$1"
  shift
  local payload
  payload="$(python3 - "$@" <<'PY'
import json
import sys
print(json.dumps({"names": sys.argv[1:]}))
PY
)"
  gh api --method PUT --header "X-GitHub-Api-Version: $API_VERSION" \
    -H 'Accept: application/vnd.github+json' \
    "repos/$full_name/topics" --input - <<<"$payload" --silent
}

stage=source-snapshot
source_bare="$work/source.git"
git clone --mirror "https://github.com/$SOURCE_REPOSITORY.git" "$source_bare"
source_main="$(git --git-dir="$source_bare" rev-parse refs/heads/main)"
[[ "$source_main" =~ ^[0-9a-f]{40}$ ]]
printf 'SOURCE_MAIN %s %s\n' "$SOURCE_REPOSITORY" "$source_main"
printf 'bootstrap-stage=%s status=passed\n' "$stage"

stage=canonical-repository
ensure_public_repository \
  'ores-otel' \
  'ores.otel.log' \
  'Canonical polyglot structured logging and OpenTelemetry SDKs; successor to ORESoftware/next-loggers.ts.'

if ! remote_has_main "$CANONICAL_REPOSITORY"; then
  git --git-dir="$source_bare" remote add canonical \
    "https://github.com/$CANONICAL_REPOSITORY.git"
  git --git-dir="$source_bare" push --mirror canonical
  printf 'MIRRORED_HISTORY %s -> %s source_main=%s\n' \
    "$SOURCE_REPOSITORY" "$CANONICAL_REPOSITORY" "$source_main"
else
  printf 'PRESERVE_EXISTING_MAIN %s\n' "$CANONICAL_REPOSITORY"
fi

canonical_work="$work/canonical"
git clone "https://github.com/$CANONICAL_REPOSITORY.git" "$canonical_work"
git -C "$canonical_work" remote set-url origin \
  "https://github.com/$CANONICAL_REPOSITORY.git"
if git -C "$canonical_work" remote get-url legacy >/dev/null 2>&1; then
  git -C "$canonical_work" remote set-url legacy \
    "https://github.com/$SOURCE_REPOSITORY.git"
else
  git -C "$canonical_work" remote add legacy \
    "https://github.com/$SOURCE_REPOSITORY.git"
fi
git -C "$canonical_work" fetch --no-tags legacy main
git -C "$canonical_work" merge-base --is-ancestor "$source_main" HEAD

python3 - "$canonical_work" <<'PY'
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

root = Path(sys.argv[1])
legacy_web = "https://github.com/ORESoftware/next-loggers.ts"
canonical_web = "https://github.com/ores-otel/ores.otel.log"
replacements = (
    ("git+https://github.com/ORESoftware/next-loggers.ts.git", f"git+{canonical_web}.git"),
    ("git+https://github.com/oresoftware/next-loggers.ts.git", f"git+{canonical_web}.git"),
    ("https://github.com/ORESoftware/next-loggers.ts.git", f"{canonical_web}.git"),
    ("https://github.com/oresoftware/next-loggers.ts.git", f"{canonical_web}.git"),
    (legacy_web, canonical_web),
    ("https://github.com/oresoftware/next-loggers.ts", canonical_web),
)

package_path = root / "package.json"
package = json.loads(package_path.read_text(encoding="utf-8"))
package["repository"] = {"type": "git", "url": f"git+{canonical_web}.git"}
package["homepage"] = f"{canonical_web}#readme"
package["bugs"] = {"url": f"{canonical_web}/issues"}
package_path.write_text(json.dumps(package, indent=2) + "\n", encoding="utf-8")

tracked = subprocess.check_output(["git", "-C", str(root), "ls-files", "-z"]).split(b"\0")
for raw_path in tracked:
    if not raw_path:
        continue
    path = root / raw_path.decode("utf-8")
    if not path.is_file():
        continue
    data = path.read_bytes()
    if b"\0" in data:
        continue
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        continue
    updated = text
    for old, new in replacements:
        updated = updated.replace(old, new)
    if updated != text:
        path.write_text(updated, encoding="utf-8")

readme = root / "README.md"
marker = "<!-- ores-otel-canonical -->"
banner = (
    f"{marker}\n"
    "> **Canonical repository:** [`ores-otel/ores.otel.log`]"
    "(https://github.com/ores-otel/ores.otel.log). "
    "`ORESoftware/next-loggers.ts` remains the legacy compatibility remote.\n\n"
)
readme_text = readme.read_text(encoding="utf-8")
if marker not in readme_text:
    readme.write_text(banner + readme_text, encoding="utf-8")

(root / "MIGRATION.md").write_text(
    "# Canonical repository migration\n\n"
    "The canonical upstream is now `https://github.com/ores-otel/ores.otel.log.git`.\n\n"
    "The preserved legacy remote is `https://github.com/ORESoftware/next-loggers.ts.git`.\n\n"
    "For an existing clone:\n\n"
    "```sh\n"
    "git remote rename origin legacy\n"
    "git remote add origin https://github.com/ores-otel/ores.otel.log.git\n"
    "git fetch --all --prune --tags\n"
    "git branch --set-upstream-to=origin/main main\n"
    "```\n\n"
    "The new repository was initialized from the complete legacy Git history, including branches and tags.\n",
    encoding="utf-8",
)

remotes_doc = root / "docs" / "REMOTES.md"
remotes_doc.parent.mkdir(parents=True, exist_ok=True)
remotes_doc.write_text(
    "# Repository remotes\n\n"
    "| Remote | URL | Role |\n"
    "| --- | --- | --- |\n"
    "| `origin` | `https://github.com/ores-otel/ores.otel.log.git` | Canonical development and releases |\n"
    "| `legacy` | `https://github.com/ORESoftware/next-loggers.ts.git` | Compatibility mirror and historical links |\n",
    encoding="utf-8",
)
PY

git -C "$canonical_work" config user.name 'ORESoftware repository automation'
git -C "$canonical_work" config user.email 'bot@oresoftware.dev'
git -C "$canonical_work" add -A
if ! git -C "$canonical_work" diff --cached --quiet; then
  git -C "$canonical_work" commit -m 'chore: make ores-otel the canonical remote'
  git -C "$canonical_work" push origin HEAD:refs/heads/main
fi
canonical_main="$(git -C "$canonical_work" rev-parse HEAD)"
[[ "$canonical_main" =~ ^[0-9a-f]{40}$ ]]
git -C "$canonical_work" merge-base --is-ancestor "$source_main" "$canonical_main"

gh api --method PATCH --header "X-GitHub-Api-Version: $API_VERSION" \
  "repos/$CANONICAL_REPOSITORY" \
  -f description='Canonical polyglot structured logging and OpenTelemetry SDKs; successor to ORESoftware/next-loggers.ts.' \
  -f homepage='https://github.com/ores-otel/ores.otel.log' \
  -f default_branch='main' \
  --silent
set_topics "$CANONICAL_REPOSITORY" \
  logging opentelemetry otel observability typescript rust go gleam dart

gh api --method PATCH --header "X-GitHub-Api-Version: $API_VERSION" \
  "repos/$SOURCE_REPOSITORY" \
  -f description='Legacy compatibility remote. Canonical development moved to ores-otel/ores.otel.log.' \
  -f homepage='https://github.com/ores-otel/ores.otel.log' \
  --silent
set_topics "$SOURCE_REPOSITORY" logging opentelemetry otel legacy compatibility

printf 'CANONICAL_MAIN %s %s\n' "$CANONICAL_REPOSITORY" "$canonical_main"
printf 'bootstrap-stage=%s status=passed\n' "$stage"
