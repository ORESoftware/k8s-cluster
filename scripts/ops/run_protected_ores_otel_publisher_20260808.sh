#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly trusted_sha="${1:?trusted k8s-cluster SHA required}"
readonly api_version='2022-11-28'
readonly expected_master='9a2ca7840150e558e9b03f4f677d28ba0c43580d'
readonly expected_part1='9c599bb13de986cb2c188a73bf43dfaf952f060b'
readonly expected_part2='d33ef865aee5be3bbd2d99c4bc26eb90f7604cf5'
readonly expected_part3='24e28b0a5df6ed77f3a3daf02f010c5d4f48d2b7'

[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
stage=initialization
work="$(mktemp -d /tmp/ores-otel-protected-publisher.XXXXXX)"
resolved_token=''
credential_source=''

cleanup() {
  unset resolved_token raw_token encoded_token secret_json
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  unset GIT_CONFIG_COUNT GIT_CONFIG_KEY_0 GIT_CONFIG_VALUE_0
  rm -rf "$work"
}
report_failure() {
  local rc=$?
  trap - ERR
  printf 'ores-otel-publisher stage=%s status=failed rc=%s\n' "$stage" "$rc" >&2
  exit "$rc"
}
trap cleanup EXIT INT TERM
trap report_failure ERR

valid_token() {
  local candidate="${1:-}"
  [[ ${#candidate} -ge 20 ]] || return 1
  [[ "$candidate" != *$'\n'* ]] || return 1
  [[ "$candidate" != *$'\r'* ]] || return 1
  [[ "$candidate" != *$'\t'* ]] || return 1
  [[ "$candidate" != *' '* ]] || return 1
}

stage=protected-credential
publisher_region="${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}"
if command -v aws >/dev/null 2>&1; then
  secret_json="$(
    aws secretsmanager get-secret-value \
      --region "$publisher_region" \
      --secret-id dd/remote-dev/agent-secrets \
      --query SecretString \
      --output text 2>/dev/null || true
  )"
  if [[ -n "$secret_json" ]]; then
    raw_token="$(
      printf '%s' "$secret_json" | python3 -c '
import json
import sys
try:
    value = json.load(sys.stdin).get("GH_PAT")
except (json.JSONDecodeError, OSError, AttributeError):
    value = None
if isinstance(value, str):
    sys.stdout.write(value)
' 2>/dev/null || true
    )"
    if valid_token "$raw_token"; then
      resolved_token="$raw_token"
      credential_source=aws-secrets-manager
    fi
  fi
fi
unset raw_token secret_json

if [[ -z "$resolved_token" ]] && command -v kubectl >/dev/null 2>&1; then
  for kubeconfig in \
    /etc/kubernetes/admin.conf \
    /root/.kube/config \
    /home/ec2-user/.kube/config
  do
    [[ -r "$kubeconfig" ]] || continue
    encoded_token="$(
      KUBECONFIG="$kubeconfig" \
        kubectl -n default get secret dd-agent-secrets \
        -o jsonpath='{.data.GH_PAT}' 2>/dev/null || true
    )"
    [[ -n "$encoded_token" ]] || continue
    raw_token="$(printf '%s' "$encoded_token" | base64 --decode 2>/dev/null || true)"
    unset encoded_token
    if valid_token "$raw_token"; then
      resolved_token="$raw_token"
      credential_source="kubernetes-secret:${kubeconfig}"
      break
    fi
  done
fi
unset raw_token encoded_token

if [[ -z "$resolved_token" ]] && \
   command -v sudo >/dev/null 2>&1 && \
   command -v getent >/dev/null 2>&1; then
  ec2_home="$(getent passwd ec2-user | awk -F: '$1 == "ec2-user" {print $6}')"
  case "$ec2_home" in
    /*)
      raw_token="$(
        sudo -u ec2-user -H env \
          -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN \
          -u GITHUB_REPOSITORY_ADMIN_TOKEN -u GH_CONFIG_DIR \
          HOME="$ec2_home" XDG_CONFIG_HOME="$ec2_home/.config" \
          bash -c 'command -v gh >/dev/null 2>&1 && gh auth token --hostname github.com' \
          2>/dev/null || true
      )"
      ;;
    *) raw_token='' ;;
  esac
  if valid_token "$raw_token"; then
    resolved_token="$raw_token"
    credential_source=protected-gh-profile
  fi
fi
unset raw_token ec2_home

if [[ -z "$resolved_token" ]]; then
  printf 'ores-otel-publisher stage=%s status=failed reason=no-readable-protected-github-credential\n' "$stage" >&2
  exit 65
fi
export GH_TOKEN="$resolved_token"
export GITHUB_REPOSITORY_ADMIN_TOKEN="$resolved_token"
unset resolved_token
printf 'ores-otel-publisher stage=%s status=passed source=%s\n' "$stage" "$credential_source"

stage=github-cli
if ! command -v gh >/dev/null 2>&1; then
  command -v python3 >/dev/null
  command -v tar >/dev/null
  cli_tag="$(python3 - <<'PY'
import json
import urllib.request
request = urllib.request.Request(
    'https://api.github.com/repos/cli/cli/releases/latest',
    headers={'Accept': 'application/vnd.github+json', 'User-Agent': 'ores-otel-protected-publisher'},
)
with urllib.request.urlopen(request, timeout=30) as response:
    print(json.load(response)['tag_name'])
PY
  )"
  cli_version="${cli_tag#v}"
  case "$(uname -m)" in
    x86_64) cli_arch=amd64 ;;
    aarch64|arm64) cli_arch=arm64 ;;
    *) printf 'unsupported architecture: %s\n' "$(uname -m)" >&2; exit 1 ;;
  esac
  cli_archive="$work/gh.tar.gz"
  cli_url="https://github.com/cli/cli/releases/download/$cli_tag/gh_${cli_version}_linux_${cli_arch}.tar.gz"
  python3 - "$cli_url" "$cli_archive" <<'PY'
import sys
import urllib.request
urllib.request.urlretrieve(sys.argv[1], sys.argv[2])
PY
  tar -xzf "$cli_archive" -C "$work"
  export PATH="$work/gh_${cli_version}_linux_${cli_arch}/bin:$PATH"
fi
command -v gh >/dev/null
printf 'ores-otel-publisher stage=%s status=passed\n' "$stage"

stage=credential-transport
askpass="$work/git-askpass.sh"
cat > "$askpass" <<'ASKPASS'
#!/usr/bin/env sh
case "${1:-}" in
  *Username*) printf '%s\n' x-access-token ;;
  *Password*) printf '%s\n' "${GH_TOKEN:?GH_TOKEN is required}" ;;
  *) exit 1 ;;
esac
ASKPASS
chmod 700 "$askpass"
export GIT_ASKPASS="$askpass"
export GIT_ASKPASS_REQUIRE=force
export GIT_TERMINAL_PROMPT=0
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=credential.helper
export GIT_CONFIG_VALUE_0=
printf 'ores-otel-publisher stage=%s status=passed\n' "$stage"

stage=identity-and-ownership
test "$(gh api --header "X-GitHub-Api-Version: $api_version" user --jq .login)" = ORESoftware
for organization in ores-otel ores-otel-test; do
  membership="$(
    gh api --header "X-GitHub-Api-Version: $api_version" \
      "user/memberships/orgs/$organization" \
      --jq '.role + ":" + .state'
  )"
  test "$membership" = admin:active
  printf 'VERIFIED_OWNER %s\n' "$organization"
done
printf 'ores-otel-publisher stage=%s status=passed\n' "$stage"

stage=trusted-control-source
control="$work/k8s-cluster"
git init "$control"
git -C "$control" remote add origin https://github.com/ORESoftware/k8s-cluster.git
git -C "$control" fetch --depth=1 origin "$trusted_sha"
git -C "$control" checkout --detach FETCH_HEAD
test "$(git -C "$control" rev-parse HEAD)" = "$trusted_sha"
printf 'ores-otel-publisher stage=%s status=passed sha=%s\n' "$stage" "$trusted_sha"

stage=publisher-validation
master='scripts/ops/bootstrap_ores_otel_repositories_20260808.sh'
part1='scripts/ops/bootstrap_ores_otel_repositories_20260808_part1.sh'
part2='scripts/ops/bootstrap_ores_otel_repositories_20260808_part2.sh'
part3='scripts/ops/bootstrap_ores_otel_repositories_20260808_part3.sh'
test "$(git -C "$control" rev-parse "HEAD:$master")" = "$expected_master"
test "$(git -C "$control" rev-parse "HEAD:$part1")" = "$expected_part1"
test "$(git -C "$control" rev-parse "HEAD:$part2")" = "$expected_part2"
test "$(git -C "$control" rev-parse "HEAD:$part3")" = "$expected_part3"
for script in "$master" "$part1" "$part2" "$part3"; do
  test "$(git -C "$control" hash-object "$script")" = "$(git -C "$control" rev-parse "HEAD:$script")"
  bash -n "$control/$script"
done
printf 'ores-otel-publisher stage=%s status=passed\n' "$stage"

stage=bounded-publication
cd "$control"
bash "$master"
printf 'ores-otel-publisher stage=%s status=passed\n' "$stage"

stage=complete
printf 'ores-otel-publisher stage=%s status=success\n' "$stage"
