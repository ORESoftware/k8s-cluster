#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly trusted_sha="${1:?trusted k8s-cluster SHA required}"
readonly publisher_path='scripts/ops/bootstrap_ores_otel_test_fleet_git_only_20260808.sh'
readonly envelope_path='scripts/ops/envelopes/ores_otel_git_only_publisher_20260808_v4.json'
readonly expected_publisher='b7a4bc2846969504f7c2e1c61ddab9f6a0e01076'
readonly expected_patched_publisher='f6cd56ad771e2ecf4f8a6374aff10184bf914ff8'
readonly expected_envelope='b8e1b70e90143a1b56077813c70f25092d6afbb1'
readonly key_id='ores-otel-20260808-v4'
readonly private_key="/var/lib/oresoftware/ores-otel-publisher/${key_id}.pem"

[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
stage=initialization
work="$(mktemp -d /tmp/ores-otel-git-only-publisher-v4.XXXXXX)"

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
  printf 'git-only-v4-stage=%s status=failed rc=%s\n' "$stage" "$rc" >&2
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

remote_main() {
  local repository="$1"
  local sha
  sha="$(
    git ls-remote --exit-code "https://github.com/${repository}.git" refs/heads/main \
      | awk 'NR == 1 {print $1}'
  )"
  [[ "$sha" =~ ^[0-9a-f]{40}$ ]]
  printf '%s' "$sha"
}

stage=trusted-control-source
control="$work/k8s-cluster"
git init "$control"
git -C "$control" remote add origin https://github.com/ORESoftware/k8s-cluster.git
git -C "$control" fetch --depth=1 origin "$trusted_sha"
git -C "$control" checkout --detach FETCH_HEAD
test "$(git -C "$control" rev-parse HEAD)" = "$trusted_sha"
printf 'git-only-v4-stage=%s status=passed sha=%s\n' "$stage" "$trusted_sha"

stage=publisher-validation
test "$(git -C "$control" rev-parse "HEAD:$publisher_path")" = "$expected_publisher"
test "$(git -C "$control" rev-parse "HEAD:$envelope_path")" = "$expected_envelope"
test "$(git -C "$control" hash-object "$publisher_path")" = "$expected_publisher"
test "$(git -C "$control" hash-object "$envelope_path")" = "$expected_envelope"
bash -n "$control/$publisher_path"
printf 'git-only-v4-stage=%s status=passed\n' "$stage"

stage=encrypted-envelope
mapfile -t envelope_fields < <(
  python3 - "$control/$envelope_path" <<'PY'
import json
import sys
from pathlib import Path

expected_targets = [
    "ores-otel-test/ores-otel-log-nodejs-test",
    "ores-otel-test/ores-otel-log-python-test",
    "ores-otel-test/ores-otel-log-go-test",
    "ores-otel-test/ores-otel-log-rust-test",
    "ores-otel-test/ores-otel-log-java-test",
    "ores-otel-test/ores-otel-log-dart-test",
    "ores-otel-test/ores-otel-log-ruby-test",
    "ores-otel-test/ores-otel-log-gleam-test",
    "ores-otel-test/ores-otel-log-erlang-test",
    "ores-otel-test/ores-otel-log-elixir-test",
    "ores-otel-test/ores-otel-log-wasm-test",
    "ores-otel-test/.github",
]
record = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert record["schema_version"] == 1
assert record["purpose"] == "one-time ORES OTEL Git-only test fleet publication"
assert record["key_id"] == "ores-otel-20260808-v4"
assert record["algorithm"] == "RSA-OAEP-SHA256"
assert record["targets"] == expected_targets
assert record["delete_after_use"] is True
for field in ("algorithm", "public_key_sha256", "ciphertext_sha256", "ciphertext_b64"):
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
printf 'git-only-v4-stage=%s status=passed key_id=%s algorithm=%s\n' \
  "$stage" "$key_id" "$algorithm"

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
printf 'git-only-v4-stage=%s status=passed\n' "$stage"

stage=git-identity-and-scope
legacy_main="$(remote_main ORESoftware/next-loggers.ts)"
canonical_main="$(remote_main ores-otel/ores.otel.log)"
node_test_main="$(remote_main ores-otel-test/ores-otel-log-nodejs-test)"
profile_main="$(remote_main ores-otel-test/.github)"
test "$legacy_main" = 05f14768232b770dfc2bbe03f27b388f5a701c74
test "$canonical_main" = 79759db06e2b34d1c270b14784801fee64080453
printf 'VERIFIED_GIT_ACCESS legacy=%s canonical=%s node_test=%s profile=%s\n' \
  "$legacy_main" "$canonical_main" "$node_test_main" "$profile_main"
printf 'git-only-v4-stage=%s status=passed\n' "$stage"

stage=publisher-hotfix
python3 - "$control/$publisher_path" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
marker = 'test_both = f"""'
index = text.find(marker)
assert index >= 0
head, tail = text[:index], text[index:]
old = '''[[ "$legacy_resolved" =~ ^[0-9a-f]{40}$ ]]
[[ "$canonical_resolved" =~ ^[0-9a-f]{40}$ ]]
'''
new = '''[[ "$legacy_resolved" =~ ^[0-9a-f]{{40}}$ ]]
[[ "$canonical_resolved" =~ ^[0-9a-f]{{40}}$ ]]
'''
assert old in tail
tail = tail.replace(old, new, 1)
path.write_text(head + tail, encoding="utf-8")
PY
test "$(git -C "$control" hash-object "$publisher_path")" = "$expected_patched_publisher"
bash -n "$control/$publisher_path"
printf 'git-only-v4-stage=%s status=passed patched_blob=%s\n' \
  "$stage" "$expected_patched_publisher"

stage=bounded-publication
cd "$control"
bash "$publisher_path"
printf 'git-only-v4-stage=%s status=passed\n' "$stage"

stage=complete
printf 'git-only-v4-stage=%s status=success\n' "$stage"
