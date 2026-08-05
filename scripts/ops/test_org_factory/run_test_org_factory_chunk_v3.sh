#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

: "${GH_TOKEN:?workflow token required}"
: "${GITHUB_EVENT_COMMENT_BODY:?trigger body required}"
: "${GITHUB_RUN_ID:?run id required}"
: "${GITHUB_WORKSPACE:?workspace required}"
: "${RUNNER_TEMP:?runner temp required}"

REPOSITORY="${REPOSITORY:-ORESoftware/k8s-cluster}"
CARRIER_NUMBER="${CARRIER_NUMBER:-916}"
trigger="$GITHUB_EVENT_COMMENT_BODY"
prefix="ops-publish-test-org-factory:${CARRIER_NUMBER}:v3:"
workflow_token="$GH_TOKEN"
bootstrap_branch="agent/bootstrap-test-portfolio"
selection_args=()
selection_label=''
expected=0
stage=parse-trigger

if [[ "$trigger" =~ ^${prefix}range:([0-9]+)-([0-9]+)$ ]]; then
  start_index="${BASH_REMATCH[1]}"
  end_index="${BASH_REMATCH[2]}"
  (( start_index >= 1 && end_index <= 182 && start_index <= end_index ))
  (( end_index - start_index + 1 <= 25 ))
  expected=$((end_index - start_index + 1))
  selection_args=(--start-index "$start_index" --end-index "$end_index")
  selection_label="range-${start_index}-${end_index}"
elif [[ "$trigger" =~ ^${prefix}indices:([0-9]+(,[0-9]+){0,9})$ ]]; then
  indices="${BASH_REMATCH[1]}"
  IFS=',' read -r -a index_items <<<"$indices"
  declare -A seen=()
  for item in "${index_items[@]}"; do
    (( item >= 1 && item <= 182 ))
    [[ -z "${seen[$item]:-}" ]]
    seen[$item]=1
  done
  expected="${#index_items[@]}"
  (( expected >= 1 && expected <= 10 ))
  selection_args=(--indices "$indices")
  selection_label="indices-${indices//,/-}"
  bootstrap_branch="agent/bootstrap-test-portfolio-v3-repair"
else
  echo "unsupported trigger" >&2
  exit 2
fi

work="$(mktemp -d "$RUNNER_TEMP/test-org-owner-broker-v3.XXXXXX")"
owner_token=''
actor=''
membership=''

cleanup() {
  unset owner_token actor membership GH_TOKEN GITHUB_TOKEN
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  find "$work" -type f -exec sh -c 'for file do : > "$file"; done' sh {} + 2>/dev/null || true
  rm -rf "$work"
}

report_failure() {
  status=$?
  trap - ERR
  GH_TOKEN="$workflow_token" gh api --method POST \
    "repos/${REPOSITORY}/issues/${CARRIER_NUMBER}/comments" \
    -f body="Paired test-organization chunk \`${selection_label}\` failed at bounded stage \`${stage}\`. No plaintext owner credential was committed, uploaded, written to Actions outputs, placed in a Git remote, or retained after the job." >/dev/null || true
  exit "$status"
}

trap cleanup EXIT
trap report_failure ERR

stage=challenge-bootstrap
private_key="$work/private.pem"
public_key="$work/public.pem"
ciphertext_file="$work/ciphertext.bin"
openssl genpkey -quiet -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out "$private_key"
chmod 600 "$private_key"
openssl pkey -in "$private_key" -pubout -out "$public_key"
nonce="$(openssl rand -hex 24)"
[[ "$nonce" =~ ^[0-9a-f]{48}$ ]]

challenge_body="$work/challenge.md"
challenge_marker="test-org-factory-credential-challenge-v3:${GITHUB_RUN_ID}:${nonce}"
response_marker="<!-- test-org-factory-credential-response-v3:${GITHUB_RUN_ID}:${nonce} -->"
{
  printf '<!-- %s -->\n' "$challenge_marker"
  printf 'One-time RSA-OAEP-SHA256/MGF1-SHA256 challenge for paired test-organization chunk `%s`. The private key exists only in this runner and is destroyed on exit.\n\n' "$selection_label"
  printf '```pem\n'
  cat "$public_key"
  printf '```\n'
} > "$challenge_body"

challenge_json="$(
  jq -n --rawfile body "$challenge_body" '{body:$body}' \
    | GH_TOKEN="$workflow_token" gh api --method POST \
        "repos/${REPOSITORY}/issues/${CARRIER_NUMBER}/comments" --input -
)"
challenge_id="$(jq -er '.id | select(type == "number" and . > 0)' <<<"$challenge_json")"

stage=await-encrypted-response
response_body=''
for _ in $(seq 1 240); do
  comments="$(GH_TOKEN="$workflow_token" gh api --paginate \
    "repos/${REPOSITORY}/issues/${CARRIER_NUMBER}/comments?per_page=100" --slurp)"
  response_body="$(
    jq -r \
      --arg marker "$response_marker" \
      --argjson challenge_id "$challenge_id" '
      [
        .[][]
        | select(.id > $challenge_id)
        | select(.user.login == "ORESoftware")
        | select(.body | startswith($marker + "\n"))
      ]
      | sort_by(.id)
      | last
      | .body // empty
    ' <<<"$comments"
  )"
  test -n "$response_body" && break
  sleep 5
done
test -n "$response_body"
test "$(grep -c '^ciphertext-base64=' <<<"$response_body")" -eq 1
ciphertext="$(sed -n 's/^ciphertext-base64=//p' <<<"$response_body")"
[[ "$ciphertext" =~ ^[A-Za-z0-9+/=]+$ ]]
test "${#ciphertext}" -le 8192
printf '%s' "$ciphertext" | base64 --decode > "$ciphertext_file"
test -s "$ciphertext_file"

stage=decrypt-ciphertext
owner_token="$(
  openssl pkeyutl -decrypt \
    -inkey "$private_key" \
    -in "$ciphertext_file" \
    -pkeyopt rsa_padding_mode:oaep \
    -pkeyopt rsa_oaep_md:sha256 \
    -pkeyopt rsa_mgf1_md:sha256 \
    2>/dev/null
)"

stage=validate-owner-token
test -n "$owner_token"
[[ "$owner_token" != *$'\n'* && "$owner_token" != *$'\r'* && "$owner_token" != *$'\t'* && "$owner_token" != *' '* ]]
[[ "$owner_token" == ghp_* || "$owner_token" == github_pat_* ]]
echo "::add-mask::$owner_token"
export GH_TOKEN="$owner_token"
actor="$(gh api user --jq '.login')"
test "$actor" = ORESoftware

organizations=(
  zed-pkg-test 3fa-app-test declarative-migrations-test cliptown-test
  claritas-viz-test embedded-alerts-test evento-globolo-test fiducia-cloud-test
  file-tunnel-test hypesiege-test memebank-test messaging-intel-test
  opto-sync-test quaestor-ledger-test scintilla-run-test shared-auth-test
  sonus-auris-test streempilot-test
)
for organization in "${organizations[@]}"; do
  membership="$(gh api "user/memberships/orgs/${organization}" --jq '[.state,.role] | join(":")')"
  test "$membership" = active:admin
  printf 'OWNER_VERIFIED %s\n' "$organization"
done

stage=reconstruct-and-patch-reviewed-factory
payload_dir="$GITHUB_WORKSPACE/scripts/ops/test_org_factory"
v2_patcher="$payload_dir/patch_test_org_factory_publisher.py"
v3_patcher="$payload_dir/patch_test_org_factory_publisher_v3.py"
publisher_encoded="$work/publish_test_org_factory.py.gz.b64"
publisher_script="$work/publish_test_org_factory.py"
source_encoded="$work/source.tar.gz.b64"
source_archive="$work/source.tar.gz"

printf '%s  %s\n' '3d43d185f6cba42057020b433c4e25ec139c2f19fdae83d8f4d7dfb86d741cca' "$v2_patcher" | sha256sum --check --strict
printf '%s  %s\n' 'f36ea14d494605f0f2805da13521f5c03577e5196a5c2507fe2c7303df21e1d5' "$v3_patcher" | sha256sum --check --strict

publisher_parts=("$payload_dir"/publish_test_org_factory.py.gz.b64.part-*)
source_parts=("$payload_dir"/source.tar.gz.b64.part-*)
[[ "${#publisher_parts[@]}" == 2 ]]
[[ "${#source_parts[@]}" == 7 ]]
cat "${publisher_parts[@]}" > "$publisher_encoded"
cat "${source_parts[@]}" > "$source_encoded"
printf '%s  %s\n' '161c0f3aef1756f8970c6d3a720e75f6264d9d929ccf82e9d1d133dd99fa08f0' "$publisher_encoded" | sha256sum --check --strict
printf '%s  %s\n' 'f29ce27911bf17ad07fb1f520a3283811b6310be774d3aa728ab6c712b19cb3f' "$source_encoded" | sha256sum --check --strict
base64 --decode < "$publisher_encoded" | gzip --decompress --stdout > "$publisher_script"
base64 --decode < "$source_encoded" > "$source_archive"
printf '%s  %s\n' '11eef4b3e2452ee022cda36be39a1ccb39fafbaa1190c693a5e092115359ff43' "$publisher_script" | sha256sum --check --strict
printf '%s  %s\n' 'eef10c331cc11f5e927c21cb33481cb7324f3785d73d3dac33f7f3bc74ac7b37' "$source_archive" | sha256sum --check --strict
python3 "$v2_patcher" "$publisher_script"
printf '%s  %s\n' 'e2ab2d308ec39ed96f5d911bdd55e79a995ad73310d983f99b6efd807463405b' "$publisher_script" | sha256sum --check --strict
python3 "$v3_patcher" "$publisher_script"
python3 -m py_compile "$v2_patcher" "$v3_patcher" "$publisher_script"
tar -tzf "$source_archive" >/dev/null

stage=bounded-repository-publication
export TEST_ORG_FACTORY_BOOTSTRAP_BRANCH="$bootstrap_branch"
python3 "$publisher_script" \
  --source "$source_archive" \
  --work-root "$work/publisher" \
  --workers 1 \
  --target-delay-seconds 20 \
  --materialize-submodules \
  "${selection_args[@]}"

stage=record-sanitized-result
summary="$work/publisher/summary.json"
test -s "$summary"
created="$(jq -r '.created // 0' "$summary")"
changed="$(jq -r '.changed // 0' "$summary")"
successful="$(jq -r '.successful // 0' "$summary")"
failed="$(jq -r '.failed // 0' "$summary")"
submodules="$(jq -r '.submodules_materialized // 0' "$summary")"
test "$successful" -eq "$expected"
test "$failed" -eq 0

GH_TOKEN="$workflow_token" gh api --method POST \
  "repos/${REPOSITORY}/issues/${CARRIER_NUMBER}/comments" \
  -f body="Paired test-organization chunk \`${selection_label}\` completed successfully. Managed targets: ${successful}; newly created during this pass: ${created}; repositories changed or initialized: ${changed}; real submodules materialized: ${submodules}; failures: ${failed}; branch: \`${bootstrap_branch}\`." >/dev/null

stage=complete
printf 'test-org-publisher-v3 stage=%s status=success selection=%s managed=%s created=%s changed=%s submodules=%s failed=%s branch=%s\n' \
  "$stage" "$selection_label" "$successful" "$created" "$changed" "$submodules" "$failed" "$bootstrap_branch"
