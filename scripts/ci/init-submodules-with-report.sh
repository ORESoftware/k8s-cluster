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

auth_mode="${SUBMODULE_AUTH_MODE:-https-token}"
report_path="${SUBMODULE_REPORT_PATH:-}"
report_mode="${SUBMODULE_REPORT_MODE:-truncate}"
askpass_file=""
declare -a git_config=()

if [[ -n "$report_path" ]]; then
  mkdir -p "$(dirname "$report_path")"
  case "$report_mode" in
    truncate)
      printf 'status\trepository\tpath\tcategory\tcommit\n' >"$report_path"
      ;;
    append)
      if [[ ! -s "$report_path" ]]; then
        printf 'status\trepository\tpath\tcategory\tcommit\n' >"$report_path"
      fi
      ;;
    *)
      echo "::error title=Invalid submodule report mode::SUBMODULE_REPORT_MODE must be truncate or append"
      exit 64
      ;;
  esac
fi

record_result() {
  local status="$1"
  local repository="$2"
  local path="$3"
  local category="$4"
  local commit="${5:-}"
  if [[ -n "$report_path" ]]; then
    printf '%s\t%s\t%s\t%s\t%s\n' \
      "$status" "$repository" "$path" "$category" "$commit" >>"$report_path"
  fi
}

cleanup() {
  if [[ -n "$askpass_file" && -f "$askpass_file" ]]; then
    find "$askpass_file" -maxdepth 0 -type f -delete
  fi
}
trap cleanup EXIT

case "$auth_mode" in
  ssh)
    ssh_command="$(git config --local core.sshCommand || true)"
    if [[ -z "$ssh_command" ]]; then
      echo "::error title=Submodule SSH unavailable::actions/checkout did not configure core.sshCommand"
      exit 2
    fi
    export GIT_SSH_COMMAND="$ssh_command"
    ;;
  https-token)
    if [[ -z "${K8S_SUBMODULE_TOKEN:-}" ]]; then
      echo "::error title=Submodule token missing::K8S_SUBMODULE_TOKEN is required for cross-organization HTTPS clones"
      exit 2
    fi
    export K8S_SUBMODULE_TOKEN
    export GIT_TERMINAL_PROMPT=0
    askpass_file="$(mktemp "${RUNNER_TEMP:-/tmp}/k8s-submodule-askpass.XXXXXX")"
    cat >"$askpass_file" <<'ASKPASS'
#!/usr/bin/env sh
case "${1:-}" in
  *Username*) printf '%s\n' 'x-access-token' ;;
  *Password*) printf '%s\n' "$K8S_SUBMODULE_TOKEN" ;;
  *) printf '%s\n' '' ;;
esac
ASKPASS
    chmod 700 "$askpass_file"
    export GIT_ASKPASS="$askpass_file"
    git_config=(
      -c 'url.https://github.com/.insteadOf=git@github.com:'
      -c 'url.https://github.com/.insteadOf=ssh://git@github.com/'
    )
    ;;
  *)
    echo "::error title=Unsupported submodule auth mode::SUBMODULE_AUTH_MODE must be ssh or https-token"
    exit 64
    ;;
esac

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

declare -a failures=()
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

  # `remote/libs` is the public schema/entity authority. It must not consume or
  # depend on the private-deployment SSH key merely because the rest of the
  # superproject still has private submodules. Keep the committed gitlink and
  # .gitmodules entry unchanged, but rewrite this one clone to anonymous HTTPS.
  # Other repositories retain the selected SSH or token authentication mode.
  declare -a path_git_config=("${git_config[@]}")
  if [[ "$path" == "remote/libs" && "$repository" == "ORESoftware/k8s-libs-and-shared-defs" ]]; then
    path_git_config=(
      -c 'url.https://github.com/.insteadOf=git@github.com:'
      -c 'url.https://github.com/.insteadOf=ssh://git@github.com/'
    )
    export GIT_TERMINAL_PROMPT=0
  fi

  echo "::group::submodule ${repository} (${path})"
  log_file="$(mktemp "${RUNNER_TEMP:-/tmp}/k8s-submodule-log.XXXXXX")"

  git "${path_git_config[@]}" submodule sync -- "$path" >/dev/null 2>&1 || true
  if git "${path_git_config[@]}" submodule update --init --recursive --depth 1 -- "$path" >"$log_file" 2>&1; then
    pinned="$(git ls-files --stage -- "$path" | awk '$1 == 160000 { print $2; exit }')"
    checkout="$(git -C "$path" rev-parse HEAD 2>/dev/null || true)"
    if [[ -z "$pinned" || "$checkout" != "$pinned" ]]; then
      failures+=("${repository}|${path}|pinned-commit-mismatch")
      record_result failure "$repository" "$path" pinned-commit-mismatch "$checkout"
      echo "::error title=Submodule pin mismatch::${repository} at ${path} did not resolve to the superproject gitlink"
    else
      record_result success "$repository" "$path" initialized "$checkout"
      echo "initialized ${repository} at pinned commit ${checkout:0:12}"
    fi
  else
    category="clone-failed"
    if grep -Eqi 'repository not found|not found' "$log_file"; then
      category="repository-missing-or-inaccessible"
    elif grep -Eqi 'authentication failed|invalid username or token|bad credentials|could not read Username|terminal prompts disabled' "$log_file"; then
      category="authentication-failed"
    elif grep -Eqi 'permission denied|access denied|403|write access.*not granted' "$log_file"; then
      category="permission-denied"
    elif grep -Eqi 'could not resolve host|connection timed out|failed to connect' "$log_file"; then
      category="network-failure"
    fi
    failures+=("${repository}|${path}|${category}")
    record_result failure "$repository" "$path" "$category"
    echo "::error title=Submodule unavailable::${repository} (${path}) category=${category}"
  fi

  if [[ -f "$log_file" ]]; then
    find "$log_file" -maxdepth 0 -type f -delete
  fi
  echo "::endgroup::"
done

if (( ${#failures[@]} > 0 )); then
  echo "Submodule initialization failed for ${#failures[@]} repository/repositories:" >&2
  for failure in "${failures[@]}"; do
    IFS='|' read -r repository path category <<<"$failure"
    printf '  - %s (%s): %s\n' "$repository" "$path" "$category" >&2
  done
  if [[ -n "$report_path" ]]; then
    echo "Sanitized report written to ${report_path}." >&2
  fi
  echo "No credential values or credential-bearing URLs were written to the report." >&2
  exit 1
fi

echo "Initialized ${#selected_paths[@]} submodule repository/repositories at their pinned commits."
if [[ -n "$report_path" ]]; then
  echo "Sanitized report written to ${report_path}."
fi
