#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly trusted_sha="${1:?trusted k8s-cluster SHA required}"
readonly publisher_path='scripts/ops/bootstrap_ores_otel_test_fleet_git_only_20260808.sh'
readonly envelope_path='scripts/ops/envelopes/ores_otel_git_only_publisher_20260808_v5.json'
readonly expected_publisher='b7a4bc2846969504f7c2e1c61ddab9f6a0e01076'
readonly expected_patched_publisher='f6cd56ad771e2ecf4f8a6374aff10184bf914ff8'
readonly expected_envelope='3fa2c138012abaa2a73905ebfa700371018a2f33'
readonly key_id='ores-otel-20260808-v5'
readonly expected_public_key_sha256='c995d59134150fb4785c117d5f427de2a5e715ef3d7222c3b7deb0280c0bf83d'
readonly expected_ciphertext_sha256='e26894b467f3a73870ff709d676c65440e6ddb6b008a8c8ebacb366e8297b7d7'
readonly private_key="/var/lib/oresoftware/ores-otel-publisher/${key_id}.pem"

[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
stage=initialization
work="$(mktemp -d /tmp/ores-otel-git-only-publisher-v5.XXXXXX)"

cleanup() {
  unset raw_token GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  unset GIT_CONFIG_COUNT GIT_CONFIG_KEY_0 GIT_CONFIG_VALUE_0
  rm -f "$private_key"
  rm -rf "$work"
}
report_failure() {
  local rc=$?
  trap - ERR
  printf 'git-only-v5-stage=%s status=failed rc=%s\n' "$stage" "$rc" >&2
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
printf 'git-only-v5-stage=%s status=passed sha=%s\n' "$stage" "$trusted_sha"

stage=publisher-validation
test "$(git -C "$control" rev-parse "HEAD:$publisher_path")" = "$expected_publisher"
test "$(git -C "$control" rev-parse "HEAD:$envelope_path")" = "$expected_envelope"
test "$(git -C "$control" hash-object "$publisher_path")" = "$expected_publisher"
test "$(git -C "$control" hash-object "$envelope_path")" = "$expected_envelope"
bash -n "$control/$publisher_path"
printf 'git-only-v5-stage=%s status=passed\n' "$stage"

stage=envelope-json
ciphertext="$work/token.ciphertext"
python3 - "$control/$envelope_path" "$ciphertext" <<'PY'
import base64
import hashlib
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
assert record == {
    "schema_version": 1,
    "purpose": "one-time ORES OTEL Git-only test fleet publication",
    "key_id": "ores-otel-20260808-v5",
    "algorithm": "RSA-OAEP-SHA256",
    "public_key_sha256": "c995d59134150fb4785c117d5f427de2a5e715ef3d7222c3b7deb0280c0bf83d",
    "ciphertext_sha256": "e26894b467f3a73870ff709d676c65440e6ddb6b008a8c8ebacb366e8297b7d7",
    "ciphertext_b64": record["ciphertext_b64"],
    "targets": expected_targets,
    "delete_after_use": True,
}
ciphertext = base64.b64decode(record["ciphertext_b64"], validate=True)
assert len(ciphertext) == 384
assert hashlib.sha256(ciphertext).hexdigest() == record["ciphertext_sha256"]
Path(sys.argv[2]).write_bytes(ciphertext)
PY
test -s "$ciphertext"
test "$(wc -c < "$ciphertext" | tr -d ' ')" = 384
test "$(sha256sum "$ciphertext" | awk '{print $1}')" = "$expected_ciphertext_sha256"
printf 'git-only-v5-stage=%s status=passed ciphertext_bytes=384 ciphertext_sha256=%s\n' \
  "$stage" "$expected_ciphertext_sha256"

stage=private-key-presence
test -e "$private_key"
test -f "$private_key"
test -r "$private_key"
key_mode="$(stat -c '%a' "$private_key")"
key_owner="$(stat -c '%u:%g' "$private_key")"
key_bytes="$(stat -c '%s' "$private_key")"
printf 'PRIVATE_KEY_METADATA key_id=%s mode=%s owner=%s bytes=%s\n' \
  "$key_id" "$key_mode" "$key_owner" "$key_bytes"
test "$key_mode" = 600
[[ "$key_bytes" =~ ^[0-9]+$ ]]
test "$key_bytes" -gt 1000
printf 'git-only-v5-stage=%s status=passed\n' "$stage"

stage=public-key-binding
public_key="$work/public.pem"
openssl pkey -in "$private_key" -pubout > "$public_key"
actual_public_key_sha256="$(sha256sum "$public_key" | awk '{print $1}')"
printf 'PUBLIC_KEY_BINDING expected=%s actual=%s\n' \
  "$expected_public_key_sha256" "$actual_public_key_sha256"
test "$actual_public_key_sha256" = "$expected_public_key_sha256"
printf 'git-only-v5-stage=%s status=passed\n' "$stage"

stage=credential-decryption
set +e
raw_token="$(
  openssl pkeyutl -decrypt \
    -inkey "$private_key" \
    -in "$ciphertext" \
    -pkeyopt rsa_padding_mode:oaep \
    -pkeyopt rsa_oaep_md:sha256 \
    -pkeyopt rsa_mgf1_md:sha256
)"
decrypt_rc=$?
set -e
printf 'CREDENTIAL_DECRYPTION rc=%s token_bytes=%s\n' "$decrypt_rc" "${#raw_token}"
test "$decrypt_rc" = 0
valid_token "$raw_token"
export GH_TOKEN="$raw_token"
export GITHUB_REPOSITORY_ADMIN_TOKEN="$raw_token"
unset raw_token
rm -f "$ciphertext" "$public_key"
printf 'git-only-v5-stage=%s status=passed token_bytes=%s\n' "$stage" "${#GH_TOKEN}"

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
printf 'git-only-v5-stage=%s status=passed\n' "$stage"

stage=git-identity-and-scope
legacy_main="$(remote_main ORESoftware/next-loggers.ts)"
canonical_main="$(remote_main ores-otel/ores.otel.log)"
node_test_main="$(remote_main ores-otel-test/ores-otel-log-nodejs-test)"
profile_main="$(remote_main ores-otel-test/.github)"
test "$legacy_main" = 05f14768232b770dfc2bbe03f27b388f5a701c74
test "$canonical_main" = 79759db06e2b34d1c270b14784801fee64080453
printf 'VERIFIED_GIT_ACCESS legacy=%s canonical=%s node_test=%s profile=%s\n' \
  "$legacy_main" "$canonical_main" "$node_test_main" "$profile_main"
printf 'git-only-v5-stage=%s status=passed\n' "$stage"

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
printf 'git-only-v5-stage=%s status=passed patched_blob=%s\n' \
  "$stage" "$expected_patched_publisher"

stage=bounded-publication
cd "$control"
bash "$publisher_path"
printf 'git-only-v5-stage=%s status=passed\n' "$stage"

stage=complete
printf 'git-only-v5-stage=%s status=success\n' "$stage"
