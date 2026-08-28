#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly trusted_sha="${1:?trusted k8s-cluster SHA required}"
readonly api_version='2022-11-28'
readonly key_id='ores-otel-20260808-v3'
readonly master='scripts/ops/bootstrap_ores_otel_repositories_20260808.sh'
readonly part1='scripts/ops/bootstrap_ores_otel_repositories_20260808_part1.sh'
readonly part2='scripts/ops/bootstrap_ores_otel_repositories_20260808_part2.sh'
readonly part3='scripts/ops/bootstrap_ores_otel_repositories_20260808_part3.sh'
readonly envelope_path='scripts/ops/envelopes/ores_otel_publisher_20260808_v3.json'
readonly expected_master='9a2ca7840150e558e9b03f4f677d28ba0c43580d'
readonly expected_part1='9c599bb13de986cb2c188a73bf43dfaf952f060b'
readonly expected_part2='bbb282a6e507054b973e6d681fa65b0cdb6486dc'
readonly expected_part3='24e28b0a5df6ed77f3a3daf02f010c5d4f48d2b7'
readonly expected_envelope='2329d0576cd75693667b71c7e0dad4cd8f106ad4'
readonly private_key="/var/lib/oresoftware/ores-otel-publisher/${key_id}.pem"

[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
stage=initialization
work="$(mktemp -d /tmp/ores-otel-protected-publisher-v3.XXXXXX)"

cleanup() {
  unset raw_token GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset ciphertext_b64 algorithm public_key_sha256 ciphertext_sha256
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  unset GIT_CONFIG_COUNT GIT_CONFIG_KEY_0 GIT_CONFIG_VALUE_0
  rm -f "$private_key"
  rm -rf "$work"
}
report_failure() {
  local rc=$?
  trap - ERR
  printf 'ores-otel-publisher-v3 stage=%s status=failed rc=%s\n' "$stage" "$rc" >&2
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

stage=trusted-control-source
control="$work/k8s-cluster"
git init "$control"
git -C "$control" remote add origin https://github.com/ORESoftware/k8s-cluster.git
git -C "$control" fetch --depth=1 origin "$trusted_sha"
git -C "$control" checkout --detach FETCH_HEAD
test "$(git -C "$control" rev-parse HEAD)" = "$trusted_sha"
printf 'ores-otel-publisher-v3 stage=%s status=passed sha=%s\n' "$stage" "$trusted_sha"

stage=publisher-validation
test "$(git -C "$control" rev-parse "HEAD:$master")" = "$expected_master"
test "$(git -C "$control" rev-parse "HEAD:$part1")" = "$expected_part1"
test "$(git -C "$control" rev-parse "HEAD:$part2")" = "$expected_part2"
test "$(git -C "$control" rev-parse "HEAD:$part3")" = "$expected_part3"
test "$(git -C "$control" rev-parse "HEAD:$envelope_path")" = "$expected_envelope"
for script in "$master" "$part1" "$part2" "$part3"; do
  test "$(git -C "$control" hash-object "$script")" = "$(git -C "$control" rev-parse "HEAD:$script")"
  bash -n "$control/$script"
done
printf 'ores-otel-publisher-v3 stage=%s status=passed\n' "$stage"

stage=encrypted-envelope
mapfile -t envelope_fields < <(
  python3 - "$control/$envelope_path" <<'PY'
import json
import sys
from pathlib import Path
record = json.loads(Path(sys.argv[1]).read_text(encoding='utf-8'))
assert record['schema_version'] == 1
assert record['purpose'] == 'one-time ORES OTEL repository fleet publication'
assert record['key_id'] == 'ores-otel-20260808-v3'
assert record['algorithm'] == 'RSA-OAEP-SHA256'
assert record['targets'] == ['ores-otel/ores.otel.log', 'ores-otel-test/*']
assert record['delete_after_use'] is True
for field in ('algorithm', 'public_key_sha256', 'ciphertext_sha256', 'ciphertext_b64'):
    value = record[field]
    assert isinstance(value, str) and value
    print(value)
PY
)
test "${#envelope_fields[@]}" = 4
algorithm="${envelope_fields[0]}"
public_key_sha256="${envelope_fields[1]}"
ciphertext_sha256="${envelope_fields[2]}"
ciphertext_b64="${envelope_fields[3]}"
test "$algorithm" = RSA-OAEP-SHA256
[[ "$public_key_sha256" =~ ^[0-9a-f]{64}$ ]]
[[ "$ciphertext_sha256" =~ ^[0-9a-f]{64}$ ]]
test -r "$private_key"
test "$(stat -c '%a' "$private_key")" = 600

public_key="$work/public.pem"
ciphertext="$work/token.ciphertext"
openssl pkey -in "$private_key" -pubout > "$public_key"
test "$(sha256sum "$public_key" | awk '{print $1}')" = "$public_key_sha256"
printf '%s' "$ciphertext_b64" | base64 --decode > "$ciphertext"
test "$(sha256sum "$ciphertext" | awk '{print $1}')" = "$ciphertext_sha256"
raw_token="$(
  openssl pkeyutl -decrypt \
    -inkey "$private_key" \
    -in "$ciphertext" \
    -pkeyopt rsa_padding_mode:oaep \
    -pkeyopt rsa_oaep_md:sha256 \
    -pkeyopt rsa_mgf1_md:sha256
)"
valid_token "$raw_token"
export GH_TOKEN="$raw_token"
export GITHUB_REPOSITORY_ADMIN_TOKEN="$raw_token"
unset raw_token ciphertext_b64 envelope_fields
rm -f "$ciphertext" "$public_key"
printf 'ores-otel-publisher-v3 stage=%s status=passed key_id=%s algorithm=%s\n' \
  "$stage" "$key_id" "$algorithm"

stage=github-cli
if ! command -v gh >/dev/null 2>&1; then
  command -v python3 >/dev/null
  command -v tar >/dev/null
  cli_tag="$(python3 - <<'PY'
import json
import urllib.request
request = urllib.request.Request(
    'https://api.github.com/repos/cli/cli/releases/latest',
    headers={'Accept': 'application/vnd.github+json', 'User-Agent': 'ores-otel-protected-publisher-v3'},
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
printf 'ores-otel-publisher-v3 stage=%s status=passed\n' "$stage"

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
printf 'ores-otel-publisher-v3 stage=%s status=passed\n' "$stage"

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
printf 'ores-otel-publisher-v3 stage=%s status=passed\n' "$stage"

stage=bounded-publication
cd "$control"
bash "$master"
printf 'ores-otel-publisher-v3 stage=%s status=passed\n' "$stage"

stage=complete
printf 'ores-otel-publisher-v3 stage=%s status=success\n' "$stage"
