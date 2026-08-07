#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
usage: init-required-submodules.sh [--recursive] [--list] <path-or-prefix>...

Expands each selector against .gitmodules, verifies every result is a pinned
superproject gitlink, and initializes one submodule at a time. Initialization
uses K8S_SUBMODULE_TOKEN for both SSH-style and HTTPS GitHub URLs without
embedding the token in repository configuration or command output.
USAGE
}

recursive=false
list_only=false
selectors=()
while (($#)); do
  case "$1" in
    --recursive)
      recursive=true
      ;;
    --list)
      list_only=true
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    --)
      shift
      selectors+=("$@")
      break
      ;;
    -*)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
    *)
      selectors+=("$1")
      ;;
  esac
  shift
done

if ((${#selectors[@]} == 0)); then
  usage >&2
  exit 2
fi
if [[ ! -f .gitmodules ]]; then
  echo "submodule bootstrap requires the repository root (.gitmodules missing)" >&2
  exit 2
fi

mapfile -t declared_paths < <(
  git config --file .gitmodules --get-regexp '^submodule\..*\.path$' 2>/dev/null \
    | awk '{print $2}' \
    | LC_ALL=C sort -u
)
if ((${#declared_paths[@]} == 0)); then
  echo "no submodules are declared in .gitmodules" >&2
  exit 2
fi

declare -A selected=()
for selector in "${selectors[@]}"; do
  matched=false
  for path in "${declared_paths[@]}"; do
    if [[ "$path" == "$selector" || "$path" == "$selector/"* ]]; then
      selected["$path"]=1
      matched=true
    fi
  done
  if [[ "$matched" != true ]]; then
    echo "selector matched no declared submodules: $selector" >&2
    exit 2
  fi
done

mapfile -t paths < <(printf '%s\n' "${!selected[@]}" | LC_ALL=C sort)
for path in "${paths[@]}"; do
  expected="$({ git ls-files --stage -- "$path" || true; } \
    | awk '$1 == "160000" { print $2; exit }')"
  if [[ ! "$expected" =~ ^[0-9a-f]{40}$ ]]; then
    echo "declared submodule is not stored as a pinned gitlink: $path" >&2
    exit 2
  fi
  if [[ "$list_only" == true ]]; then
    printf '%s\n' "$path"
  fi
done
if [[ "$list_only" == true ]]; then
  exit 0
fi

: "${K8S_SUBMODULE_TOKEN:?K8S_SUBMODULE_TOKEN is required}"

# Fail closed on credentials known to have appeared in diagnostic output. Only
# one-way fingerprints are stored here; the credential material is never logged.
token_sha256="$(printf '%s' "$K8S_SUBMODULE_TOKEN" | sha256sum | awk '{print $1}')"
revoked_token_sha256=(
  fc315dce30048ea9efc260d3d76fb5939113bab4e62c449c30d573457c08f908
)
for revoked in "${revoked_token_sha256[@]}"; do
  if [[ "$token_sha256" == "$revoked" ]]; then
    echo "::error title=Rotate REMOTE_DEV_GH_PAT::The configured submodule token is revoked and must be replaced before repository access." >&2
    exit 1
  fi
done
unset token_sha256 revoked

# Use a private, ephemeral askpass helper rather than embedding credentials in a
# URL, Git config, command argument, or derived masking command. The token stays
# in the existing secret environment variable and is returned only to Git's
# credential prompt. Every nested Git process inherits this environment.
auth_dir="$(mktemp -d)"
chmod 700 "$auth_dir"
askpass="$auth_dir/git-askpass.sh"
cat > "$askpass" <<'ASKPASS'
#!/usr/bin/env bash
case "${1:-}" in
  *Username*|*username*)
    printf '%s\n' 'x-access-token'
    ;;
  *Password*|*password*)
    printf '%s\n' "${K8S_SUBMODULE_TOKEN:?K8S_SUBMODULE_TOKEN is required}"
    ;;
  *)
    printf '\n'
    ;;
esac
ASKPASS
chmod 700 "$askpass"
cleanup_auth() {
  rm -rf "$auth_dir"
}
trap cleanup_auth EXIT
export GIT_ASKPASS="$askpass"
export GIT_ASKPASS_REQUIRE=force
export GIT_TERMINAL_PROMPT=0
export GCM_INTERACTIVE=never

# These process-local URL rewrites are inherited by every nested Git process.
# checkout uses persist-credentials:false, so askpass is the sole auth source.
export GIT_CONFIG_COUNT=2
export GIT_CONFIG_KEY_0='url.https://github.com/.insteadOf'
export GIT_CONFIG_VALUE_0='git@github.com:'
export GIT_CONFIG_KEY_1='url.https://github.com/.insteadOf'
export GIT_CONFIG_VALUE_1='ssh://git@github.com/'

scrub_output() {
  sed -E \
    -e 's#https://x-access-token:[^@[:space:]]+@github\.com/#https://github.com/#g' \
    -e 's#(AUTHORIZATION: basic )[A-Za-z0-9+/=]+#\1***#g'
}

classify_failure() {
  local output="$1"
  if grep -Eqi 'repository not found|authentication failed|could not read username|permission denied' <<<"$output"; then
    printf '%s' 'auth_or_visibility'
  elif grep -Eqi 'not our ref|did not contain|reference is not a tree|unadvertised object' <<<"$output"; then
    printf '%s' 'pinned_commit_unavailable'
  else
    printf '%s' 'git_submodule_error'
  fi
}

group_start() {
  if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
    echo "::group::submodule $1"
  else
    echo "==> submodule $1"
  fi
}

group_end() {
  if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
    echo "::endgroup::"
  fi
}

for path in "${paths[@]}"; do
  group_start "$path"
  git submodule sync --recursive -- "$path" >/dev/null

  shallow=(git submodule update --init --depth 1)
  full=(git submodule update --init)
  if [[ "$recursive" == true ]]; then
    shallow+=(--recursive)
    full+=(--recursive)
  fi
  shallow+=(-- "$path")
  full+=(-- "$path")

  if output="$("${shallow[@]}" 2>&1)"; then
    :
  elif grep -Eqi 'not our ref|did not contain|reference is not a tree|unadvertised object' <<<"$output"; then
    echo "shallow fetch could not reach the pin; retrying full fetch for $path" >&2
    if ! output="$("${full[@]}" 2>&1)"; then
      reason="$(classify_failure "$output")"
      group_end
      detail="path=${path}; reason=${reason}; verify REMOTE_DEV_GH_PAT read access and the pinned gitlink"
      echo "::error title=Submodule initialization failed::${detail}"
      scrub_output <<<"$output" | tail -n 20 >&2
      exit 1
    fi
  else
    reason="$(classify_failure "$output")"
    group_end
    detail="path=${path}; reason=${reason}; verify REMOTE_DEV_GH_PAT read access and the pinned gitlink"
    echo "::error title=Submodule initialization failed::${detail}"
    scrub_output <<<"$output" | tail -n 20 >&2
    exit 1
  fi

  expected="$(git ls-files --stage -- "$path" | awk '$1 == "160000" { print $2; exit }')"
  actual="$(git -C "$path" rev-parse HEAD)"
  if [[ "$actual" != "$expected" ]]; then
    group_end
    echo "::error title=Submodule pin mismatch::path=${path}; checkout=${actual}; pin=${expected}"
    exit 1
  fi

  if [[ "$recursive" == true ]]; then
    status="$(git submodule status --recursive -- "$path")"
    if grep -Eq '^[+-U]' <<<"$status"; then
      group_end
      echo "::error title=Nested submodule pin mismatch::path=${path}"
      printf '%s\n' "$status" >&2
      exit 1
    fi
  fi
  echo "initialized $path at $expected"
  group_end
done
