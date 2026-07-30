#!/usr/bin/env bash
set -euo pipefail

if (( $# == 0 )); then
  echo "usage: $0 <submodule-path-or-prefix> [...]" >&2
  exit 64
fi
if [[ ! -f .gitmodules ]]; then
  echo "::error title=Submodule configuration missing::.gitmodules was not found in the repository root"
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mint_script="${script_dir}/mint-github-app-installation-token.sh"
init_script="${script_dir}/init-submodules-with-report.sh"
allowlist_path="${K8S_SUBMODULE_ALLOWLIST:-${script_dir}/../../config/ci/k8s-submodule-github-app-allowlist.json}"
for script in "$mint_script" "$init_script"; do
  if [[ ! -f "$script" ]]; then
    echo "::error title=Submodule helper missing::${script} was not found"
    exit 2
  fi
done
if [[ ! -f "$allowlist_path" ]]; then
  echo "::error title=GitHub App allowlist missing::${allowlist_path} was not found"
  exit 2
fi

api_url="${GITHUB_API_URL:-https://api.github.com}"
api_version="${GITHUB_API_VERSION:-2026-03-10}"
report_path="${SUBMODULE_REPORT_PATH:-${RUNNER_TEMP:-/tmp}/backend-submodule-access.tsv}"
work_dir="$(mktemp -d "${RUNNER_TEMP:-/tmp}/github-app-submodules.XXXXXX")"
records_file="${work_dir}/submodules.tsv"

cleanup() {
  if [[ -d "$work_dir" ]]; then
    find "$work_dir" -depth -delete
  fi
}
trap cleanup EXIT

mapfile -t allowlisted_repositories < <(
  python3 - "$allowlist_path" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
document = json.loads(path.read_text(encoding="utf-8"))
if document.get("schema_version") != 1:
    raise SystemExit("GitHub App allowlist schema_version must equal 1")
permissions = document.get("required_permissions")
if permissions != {"contents": "read", "metadata": "read"}:
    raise SystemExit("GitHub App allowlist permissions must be exactly contents:read and metadata:read")
repositories_by_owner = document.get("repositories_by_owner")
if not isinstance(repositories_by_owner, dict) or not repositories_by_owner:
    raise SystemExit("GitHub App allowlist repositories_by_owner must be a non-empty object")
repositories = []
for owner, names in repositories_by_owner.items():
    if not isinstance(owner, str) or not owner.strip():
        raise SystemExit("GitHub App allowlist owner names must be non-empty strings")
    if not isinstance(names, list) or not names:
        raise SystemExit(f"GitHub App allowlist owner {owner!r} must contain repositories")
    if names != sorted(set(names)):
        raise SystemExit(f"GitHub App allowlist repositories for {owner!r} must be sorted and unique")
    for name in names:
        if not isinstance(name, str) or not name.strip() or "/" in name:
            raise SystemExit(f"GitHub App allowlist repository {owner!r}/{name!r} is invalid")
        repositories.append(f"{owner}/{name}")
for repository in sorted(repositories):
    print(repository)
PY
)
declare -A allowlisted=()
for repository in "${allowlisted_repositories[@]}"; do
  allowlisted["$repository"]=1
done

mapfile -t declared_paths < <(
  git config -f .gitmodules --get-regexp '^submodule\..*\.path$' \
    | awk '{print $2}' \
    | LC_ALL=C sort
)

declare -A selected=()
for requested in "$@"; do
  matched=false
  for path in "${declared_paths[@]}"; do
    if [[ "$path" == "$requested" || "$path" == "$requested/"* ]]; then
      selected["$path"]=1
      matched=true
    fi
  done
  if [[ "$matched" != true ]]; then
    echo "::error title=Unknown submodule path::${requested} is not a declared submodule or submodule prefix"
    exit 64
  fi
done

mapfile -t selected_paths < <(printf '%s\n' "${!selected[@]}" | LC_ALL=C sort)
: >"$records_file"
for path in "${selected_paths[@]}"; do
  path_key="$(
    git config -f .gitmodules --get-regexp '^submodule\..*\.path$' \
      | awk -v expected="$path" '$2 == expected { print $1; exit }'
  )"
  section="${path_key%.path}"
  url="$(git config -f .gitmodules --get "${section}.url")"
  repository="$url"
  repository="${repository#git@github.com:}"
  repository="${repository#ssh://git@github.com/}"
  repository="${repository#https://github.com/}"
  repository="${repository%.git}"
  if [[ ! "$repository" =~ ^([^/]+)/([^/]+)$ ]]; then
    echo "::error title=Unsupported submodule URL::${path} does not resolve to an owner/repository GitHub path"
    exit 64
  fi
  if [[ -z "${allowlisted[$repository]:-}" ]]; then
    echo "::error title=Repository not allowlisted::${repository} (${path}) is absent from ${allowlist_path}"
    exit 2
  fi
  owner="${BASH_REMATCH[1]}"
  repository_name="${BASH_REMATCH[2]}"
  printf '%s\t%s\t%s\t%s\n' "$owner" "$repository_name" "$repository" "$path" >>"$records_file"
done

mkdir -p "$(dirname "$report_path")"
printf 'status\trepository\tpath\tcategory\tcommit\n' >"$report_path"

if [[ -z "${K8S_SUBMODULE_APP_ID:-}" || -z "${K8S_SUBMODULE_APP_PRIVATE_KEY:-}" ]]; then
  while IFS=$'\t' read -r _ _ repository path; do
    printf 'failure\t%s\t%s\tgithub-app-credentials-missing\t\n' "$repository" "$path" >>"$report_path"
  done <"$records_file"
  if [[ -z "${K8S_SUBMODULE_APP_ID:-}" ]]; then
    echo "::error title=GitHub App ID missing::K8S_SUBMODULE_APP_ID is required"
  fi
  if [[ -z "${K8S_SUBMODULE_APP_PRIVATE_KEY:-}" ]]; then
    echo "::error title=GitHub App private key missing::K8S_SUBMODULE_APP_PRIVATE_KEY is required"
  fi
  echo "Sanitized report written to ${report_path}." >&2
  echo "No App JWT, installation token, private key, or credential-bearing URL was written to the report." >&2
  exit 2
fi

mapfile -t owners < <(cut -f1 "$records_file" | LC_ALL=C sort -u)
overall_status=0
for owner in "${owners[@]}"; do
  mapfile -t repositories < <(
    awk -F '\t' -v owner="$owner" '$1 == owner { print $2 }' "$records_file" | LC_ALL=C sort -u
  )
  mapfile -t repository_paths < <(
    awk -F '\t' -v owner="$owner" '$1 == owner { print $4 }' "$records_file" | LC_ALL=C sort
  )
  token_file="${work_dir}/${owner}.token"

  echo "::group::GitHub App installation for ${owner}"
  if ! bash "$mint_script" "$owner" "$token_file" "${repositories[@]}"; then
    overall_status=1
    while IFS=$'\t' read -r _ _ repository path; do
      printf 'failure\t%s\t%s\tinstallation-token-unavailable\t\n' "$repository" "$path" >>"$report_path"
    done < <(awk -F '\t' -v owner="$owner" '$1 == owner' "$records_file")
    echo "::endgroup::"
    continue
  fi
  echo "::endgroup::"

  installation_token="$(<"$token_file")"
  if [[ -f "$token_file" ]]; then
    find "$token_file" -maxdepth 0 -type f -delete
  fi

  set +e
  SUBMODULE_AUTH_MODE=https-token \
    SUBMODULE_REPORT_PATH="$report_path" \
    SUBMODULE_REPORT_MODE=append \
    K8S_SUBMODULE_TOKEN="$installation_token" \
    bash "$init_script" "${repository_paths[@]}"
  owner_status=$?
  set -e

  revoke_status="$(
    curl --silent --show-error \
      --request DELETE \
      --output /dev/null \
      --write-out '%{http_code}' \
      --header 'Accept: application/vnd.github+json' \
      --header "Authorization: Bearer ${installation_token}" \
      --header "X-GitHub-Api-Version: ${api_version}" \
      "${api_url%/}/installation/token"
  )" || revoke_status="000"
  if [[ "$revoke_status" != "204" ]]; then
    echo "::warning title=GitHub App token revocation failed::${owner} returned HTTP ${revoke_status}; the token will still expire automatically"
  fi
  unset installation_token

  if (( owner_status != 0 )); then
    overall_status=1
  fi
done

if (( overall_status != 0 )); then
  echo "One or more owner-scoped GitHub App installation batches failed." >&2
  echo "Sanitized report written to ${report_path}." >&2
  echo "No App JWT, installation token, private key, or credential-bearing URL was written to the report." >&2
  exit 1
fi

printf 'Initialized %d submodule repository/repositories across %d owner installation(s).\n' \
  "${#selected_paths[@]}" "${#owners[@]}"
echo "Sanitized report written to ${report_path}."
