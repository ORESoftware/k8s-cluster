#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly trusted_sha="${1:?trusted k8s-cluster SHA required}"
readonly publisher_path='scripts/ops/bootstrap_ores_otel_test_fleet_git_only_20260808.sh'
readonly envelope_path='scripts/ops/envelopes/ores_otel_git_only_publisher_20260808_v6.json'
readonly expected_publisher='b7a4bc2846969504f7c2e1c61ddab9f6a0e01076'
readonly expected_patched_publisher='df14edf82e486813fae3ae6395c5ec26227ff0cc'
readonly expected_envelope='0565ef9271d000e3d7a48f4988edc91d13fc517c'
readonly synchronized_main='cbc38e069c1ed4e44eb010ff430b028397fa0520'
readonly key_id='ores-otel-20260808-v6'
readonly expected_public_key_sha256='642b4a446fa547dbb12645d4e68152ac8f5d00c8cfd6be36931320debaced8b8'
readonly expected_ciphertext_sha256='63e6ebb3528de47c6a40d0d6b70d6930b9d69b3763a04c718621bb28fa5b641e'
readonly private_key="/var/lib/oresoftware/ores-otel-publisher/${key_id}.pem"

[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
stage=initialization
work="$(mktemp -d /tmp/ores-otel-git-only-v6.XXXXXX)"
cleanup() {
  unset raw_token GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  unset GIT_CONFIG_COUNT GIT_CONFIG_KEY_0 GIT_CONFIG_VALUE_0
  rm -f "$private_key"
  rm -rf "$work"
}
fail() {
  local rc=$?
  trap - ERR
  printf 'git-only-v6-stage=%s status=failed rc=%s\n' "$stage" "$rc" >&2
  exit "$rc"
}
trap cleanup EXIT INT TERM
trap fail ERR

stage=trusted-control-source
control="$work/k8s-cluster"
git init "$control"
git -C "$control" remote add origin https://github.com/ORESoftware/k8s-cluster.git
git -C "$control" fetch --depth=1 origin "$trusted_sha"
git -C "$control" checkout --detach FETCH_HEAD
test "$(git -C "$control" rev-parse HEAD)" = "$trusted_sha"
test "$(git -C "$control" rev-parse "HEAD:$publisher_path")" = "$expected_publisher"
test "$(git -C "$control" rev-parse "HEAD:$envelope_path")" = "$expected_envelope"
bash -n "$control/$publisher_path"
printf 'git-only-v6-stage=%s status=passed sha=%s\n' "$stage" "$trusted_sha"

stage=credential-decryption
ciphertext="$work/token.ciphertext"
python3 - "$control/$envelope_path" "$ciphertext" <<'PY'
import base64, hashlib, json, sys
from pathlib import Path
p = json.loads(Path(sys.argv[1]).read_text())
assert p['schema_version'] == 1
assert p['purpose'] == 'one-time ORES OTEL Git-only test fleet publication'
assert p['key_id'] == 'ores-otel-20260808-v6'
assert p['algorithm'] == 'RSA-OAEP-SHA256'
assert p['public_key_sha256'] == '642b4a446fa547dbb12645d4e68152ac8f5d00c8cfd6be36931320debaced8b8'
assert p['ciphertext_sha256'] == '63e6ebb3528de47c6a40d0d6b70d6930b9d69b3763a04c718621bb28fa5b641e'
assert len(p['targets']) == 12 and len(set(p['targets'])) == 12
assert p['delete_after_use'] is True
raw = base64.b64decode(p['ciphertext_b64'], validate=True)
assert len(raw) == 384
assert hashlib.sha256(raw).hexdigest() == p['ciphertext_sha256']
Path(sys.argv[2]).write_bytes(raw)
PY
test -r "$private_key"
test "$(stat -c '%a' "$private_key")" = 600
public_key="$work/public.pem"
openssl pkey -in "$private_key" -pubout > "$public_key"
test "$(sha256sum "$public_key" | awk '{print $1}')" = "$expected_public_key_sha256"
test "$(sha256sum "$ciphertext" | awk '{print $1}')" = "$expected_ciphertext_sha256"
raw_token="$(openssl pkeyutl -decrypt -inkey "$private_key" -in "$ciphertext" \
  -pkeyopt rsa_padding_mode:oaep -pkeyopt rsa_oaep_md:sha256 -pkeyopt rsa_mgf1_md:sha256)"
[[ ${#raw_token} -ge 20 && "$raw_token" != *$'\n'* && "$raw_token" != *' '* ]]
export GH_TOKEN="$raw_token"
export GITHUB_REPOSITORY_ADMIN_TOKEN="$raw_token"
unset raw_token
rm -f "$ciphertext" "$public_key"
printf 'git-only-v6-stage=%s status=passed token_bytes=%s\n' "$stage" "${#GH_TOKEN}"

stage=credential-transport
askpass="$work/git-askpass.sh"
cat > "$askpass" <<'ASKPASS'
#!/usr/bin/env sh
case "${1:-}" in
  *Username*) printf '%s\n' ORESoftware ;;
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
printf 'git-only-v6-stage=%s status=passed username=ORESoftware\n' "$stage"

probe_repo() {
  local repository="$1" label="$2" err="$work/${label}.stderr" output rc sha
  set +e
  output="$(git ls-remote --exit-code "https://ORESoftware@github.com/${repository}.git" refs/heads/main 2>"$err")"
  rc=$?
  set -e
  if [[ $rc -ne 0 ]]; then
    printf 'GIT_PROBE repository=%s rc=%s\n' "$repository" "$rc" >&2
    sed -E 's/ghp_[A-Za-z0-9_]+/[REDACTED]/g; s/(Authorization:).*/\1 [REDACTED]/Ig' "$err" >&2
    return "$rc"
  fi
  sha="$(awk 'NR == 1 {print $1}' <<<"$output")"
  [[ "$sha" =~ ^[0-9a-f]{40}$ ]]
  printf 'GIT_PROBE repository=%s rc=0 main=%s\n' "$repository" "$sha" >&2
  printf '%s' "$sha"
}

stage=git-scope-verification
legacy_main="$(probe_repo ORESoftware/next-loggers.ts legacy)"
canonical_main="$(probe_repo ores-otel/ores.otel.log canonical)"
test "$legacy_main" = "$synchronized_main"
test "$canonical_main" = "$synchronized_main"
readonly -a targets=(
  ores-otel-test/ores-otel-log-nodejs-test
  ores-otel-test/ores-otel-log-python-test
  ores-otel-test/ores-otel-log-go-test
  ores-otel-test/ores-otel-log-rust-test
  ores-otel-test/ores-otel-log-java-test
  ores-otel-test/ores-otel-log-dart-test
  ores-otel-test/ores-otel-log-ruby-test
  ores-otel-test/ores-otel-log-gleam-test
  ores-otel-test/ores-otel-log-erlang-test
  ores-otel-test/ores-otel-log-elixir-test
  ores-otel-test/ores-otel-log-wasm-test
  ores-otel-test/.github
)
for index in "${!targets[@]}"; do
  probe_repo "${targets[$index]}" "target-$index" >/dev/null
done
write_probe="$work/write-probe"
git clone --depth=1 --branch main "https://ORESoftware@github.com/ores-otel-test/ores-otel-log-nodejs-test.git" "$write_probe"
git -C "$write_probe" push --dry-run origin HEAD:refs/heads/ores-otel-auth-probe-v6
printf 'git-only-v6-stage=%s status=passed targets=%s dry_run_write=passed\n' "$stage" "${#targets[@]}"

stage=publisher-repin
python3 - "$control/$publisher_path" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
text = path.read_text()
text = text.replace("readonly EXPECTED_LEGACY_MAIN='05f14768232b770dfc2bbe03f27b388f5a701c74'", "readonly EXPECTED_LEGACY_MAIN='cbc38e069c1ed4e44eb010ff430b028397fa0520'", 1)
text = text.replace("readonly EXPECTED_CANONICAL_MAIN='79759db06e2b34d1c270b14784801fee64080453'", "readonly EXPECTED_CANONICAL_MAIN='cbc38e069c1ed4e44eb010ff430b028397fa0520'", 1)
text = text.replace("*Username*) printf '%s\\n' x-access-token ;;", "*Username*) printf '%s\\n' ORESoftware ;;", 1)
marker = 'test_both = f"""'
pos = text.find(marker)
assert pos >= 0
head, tail = text[:pos], text[pos:]
tail = tail.replace('[[ "$legacy_resolved" =~ ^[0-9a-f]{40}$ ]]', '[[ "$legacy_resolved" =~ ^[0-9a-f]{{40}}$ ]]', 1)
tail = tail.replace('[[ "$canonical_resolved" =~ ^[0-9a-f]{40}$ ]]', '[[ "$canonical_resolved" =~ ^[0-9a-f]{{40}}$ ]]', 1)
path.write_text(head + tail)
PY
test "$(git -C "$control" hash-object "$publisher_path")" = "$expected_patched_publisher"
bash -n "$control/$publisher_path"
printf 'git-only-v6-stage=%s status=passed patched_blob=%s\n' "$stage" "$expected_patched_publisher"

stage=bounded-publication
cd "$control"
bash "$publisher_path"
printf 'git-only-v6-stage=%s status=passed\n' "$stage"

stage=complete
printf 'git-only-v6-stage=%s status=success\n' "$stage"
