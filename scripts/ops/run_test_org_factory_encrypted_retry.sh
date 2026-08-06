#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

: "${GH_TOKEN:?workflow GitHub token is required}"
: "${GITHUB_WORKSPACE:?GitHub workspace is required}"
: "${RUNNER_TEMP:?runner temp directory is required}"

REPOSITORY="${REPOSITORY:-ORESoftware/k8s-cluster}"
CARRIER_NUMBER="${CARRIER_NUMBER:-916}"
stage=challenge-bootstrap
workflow_token="$GH_TOKEN"
work="$(mktemp -d "$RUNNER_TEMP/test-org-owner-retry.XXXXXX")"
owner_token=''
actor=''
membership=''
diagnostic_posted=false

cleanup() {
  unset owner_token actor membership GH_TOKEN GITHUB_TOKEN
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  find "$work" -type f -exec sh -c 'for file do : > "$file"; done' sh {} + 2>/dev/null || true
  rm -rf "$work"
}

report_failure() {
  status=$?
  trap - ERR
  if test "$diagnostic_posted" != true; then
    GH_TOKEN="$workflow_token" gh api --method POST \
      "repos/${REPOSITORY}/issues/${CARRIER_NUMBER}/comments" \
      -f body="Paired test-organization retry failed at bounded stage \`${stage}\`. No plaintext owner credential was committed, uploaded, written to Actions outputs, placed in a Git remote, or retained after the job." >/dev/null || true
  fi
  exit "$status"
}

trap cleanup EXIT
trap report_failure ERR

private_key="$work/private.pem"
public_key="$work/public.pem"
ciphertext_file="$work/ciphertext.bin"
openssl genpkey -quiet -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out "$private_key"
chmod 600 "$private_key"
openssl pkey -in "$private_key" -pubout -out "$public_key"
nonce="$(openssl rand -hex 24)"
[[ "$nonce" =~ ^[0-9a-f]{48}$ ]]

challenge_body="$work/challenge.md"
{
  printf '<!-- test-org-factory-credential-challenge:%s -->\n' "$nonce"
  printf 'Idempotent retry challenge using RSA-OAEP-SHA256/MGF1-SHA256. Completed repositories and pull requests are preserved; only missing or stale managed targets are changed. The private key exists only in this runner and is destroyed on exit.\n\n'
  printf '```pem\n'
  cat "$public_key"
  printf '```\n'
} > "$challenge_body"

challenge_json="$(
  jq -n --rawfile body "$challenge_body" '{body:$body}' \
    | gh api --method POST "repos/${REPOSITORY}/issues/${CARRIER_NUMBER}/comments" --input -
)"
challenge_id="$(jq -er '.id | select(type == "number" and . > 0)' <<<"$challenge_json")"
response_marker="<!-- test-org-factory-credential-response:${nonce} -->"

stage=await-encrypted-response
response_body=''
for _ in $(seq 1 360); do
  comments="$(gh api --paginate "repos/${REPOSITORY}/issues/${CARRIER_NUMBER}/comments?per_page=100" --slurp)"
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

stage=validate-owner-token-shape
test -n "$owner_token"
[[ "$owner_token" != *$'\n'* && "$owner_token" != *$'\r'* && "$owner_token" != *$'\t'* && "$owner_token" != *' '* ]]
[[ "$owner_token" == ghp_* || "$owner_token" == github_pat_* ]]
echo "::add-mask::$owner_token"
export GH_TOKEN="$owner_token"

stage=validate-owner-identity
actor="$(gh api user --jq '.login')"
test "$actor" = ORESoftware

organizations=(
  zed-pkg-test
  3fa-app-test
  declarative-migrations-test
  cliptown-test
  claritas-viz-test
  embedded-alerts-test
  evento-globolo-test
  fiducia-cloud-test
  file-tunnel-test
  hypesiege-test
  memebank-test
  messaging-intel-test
  opto-sync-test
  quaestor-ledger-test
  scintilla-run-test
  shared-auth-test
  sonus-auris-test
  streempilot-test
)
for organization in "${organizations[@]}"; do
  membership="$(gh api "user/memberships/orgs/${organization}" --jq '[.state,.role] | join(":")')"
  test "$membership" = active:admin
  printf 'OWNER_VERIFIED %s\n' "$organization"
done

stage=reconstruct-and-validate-reviewed-factory
payload_dir="$GITHUB_WORKSPACE/scripts/ops/test_org_factory"
publisher_encoded="$work/publish_test_org_factory.py.gz.b64"
publisher_script="$work/publish_test_org_factory.py"
source_encoded="$work/source.tar.gz.b64"
source_archive="$work/source.tar.gz"

publisher_parts=("$payload_dir"/publish_test_org_factory.py.gz.b64.part-*)
source_parts=("$payload_dir"/source.tar.gz.b64.part-*)
[[ "${#publisher_parts[@]}" == 2 ]]
[[ "${#source_parts[@]}" == 7 ]]
cat "${publisher_parts[@]}" > "$publisher_encoded"
cat "${source_parts[@]}" > "$source_encoded"
printf '%s  %s\n' \
  '161c0f3aef1756f8970c6d3a720e75f6264d9d929ccf82e9d1d133dd99fa08f0' \
  "$publisher_encoded" \
  | sha256sum --check --strict
printf '%s  %s\n' \
  'f29ce27911bf17ad07fb1f520a3283811b6310be774d3aa728ab6c712b19cb3f' \
  "$source_encoded" \
  | sha256sum --check --strict
base64 --decode < "$publisher_encoded" | gzip --decompress --stdout > "$publisher_script"
base64 --decode < "$source_encoded" > "$source_archive"
printf '%s  %s\n' \
  '11eef4b3e2452ee022cda36be39a1ccb39fafbaa1190c693a5e092115359ff43' \
  "$publisher_script" \
  | sha256sum --check --strict
printf '%s  %s\n' \
  'eef10c331cc11f5e927c21cb33481cb7324f3785d73d3dac33f7f3bc74ac7b37' \
  "$source_archive" \
  | sha256sum --check --strict
python3 -m py_compile "$publisher_script"
tar -tzf "$source_archive" >/dev/null

stage=bounded-repository-publication
publisher_log="$work/publisher.log"
set +e
python3 "$publisher_script" \
  --source "$source_archive" \
  --work-root "$work/publisher" \
  --workers 3 \
  --materialize-submodules \
  2>&1 | tee "$publisher_log"
publisher_rc="${PIPESTATUS[0]}"
set -e

summary="$work/publisher/summary.json"
if test "$publisher_rc" -ne 0; then
  diagnostic_body="$work/diagnostic.md"
  python3 - "$publisher_log" "$summary" > "$diagnostic_body" <<'PY'
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

log_path = Path(sys.argv[1])
summary_path = Path(sys.argv[2])
secret_pattern = re.compile(r"\b(?:ghp_|github_pat_)[A-Za-z0-9_]+\b", re.IGNORECASE)

def clean(value: object) -> str:
    text = str(value)
    text = secret_pattern.sub("[REDACTED]", text)
    return text.replace("```", "~~~")

lines = log_path.read_text(errors="replace").splitlines() if log_path.exists() else []
log_tail = "\n".join(clean(line) for line in lines[-160:])[-18000:]
summary_excerpt: dict[str, object] = {}
failed_targets: list[dict[str, str]] = []
if summary_path.exists():
    try:
        payload = json.loads(summary_path.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        summary_excerpt["summary_parse_error"] = clean(exc)
    else:
        for key in ("expected", "created", "changed", "successful", "failed"):
            if key in payload:
                summary_excerpt[key] = payload[key]
        results = payload.get("results")
        if isinstance(results, list):
            for item in results:
                if not isinstance(item, dict):
                    continue
                status = str(item.get("status", "")).lower()
                success = item.get("success")
                if success is False or status in {"failed", "failure", "error"}:
                    failed_targets.append({
                        key: clean(item[key])
                        for key in (
                            "repository_full_name",
                            "full_name",
                            "target",
                            "organization",
                            "repository",
                            "status",
                            "error",
                        )
                        if key in item and item[key] not in (None, "")
                    })
                if len(failed_targets) >= 40:
                    break

print("Paired test-organization retry failed inside the bounded publisher. Completed repositories, branches, commits, and pull requests remain intact and will be reused by the next idempotent run.")
print()
print("### Sanitized summary")
print("```json")
print(json.dumps({"counts": summary_excerpt, "failed_targets": failed_targets}, indent=2, sort_keys=True)[:22000])
print("```")
print()
print("### Sanitized publisher log tail")
print("```text")
print(log_tail or "No publisher log text was captured.")
print("```")
PY
  diagnostic_posted=true
  jq -n --rawfile body "$diagnostic_body" '{body:$body}' \
    | GH_TOKEN="$workflow_token" gh api --method POST \
        "repos/${REPOSITORY}/issues/${CARRIER_NUMBER}/comments" --input - >/dev/null
  exit "$publisher_rc"
fi

stage=record-sanitized-success
test -s "$summary"
created="$(jq -r '.created // 0' "$summary")"
changed="$(jq -r '.changed // 0' "$summary")"
successful="$(jq -r '.successful // 0' "$summary")"
failed="$(jq -r '.failed // 0' "$summary")"
test "$successful" -eq 182
test "$failed" -eq 0

GH_TOKEN="$workflow_token" gh api --method POST \
  "repos/${REPOSITORY}/issues/${CARRIER_NUMBER}/comments" \
  -f body="Paired test-organization publication completed successfully. Managed targets: ${successful}; newly created in this retry: ${created}; repositories changed or initialized in this retry: ${changed}; failures: ${failed}. All 18 owner memberships, payload hashes, remote branches, and draft pull requests passed final verification." >/dev/null

stage=complete
printf 'test-org-publisher stage=%s status=success managed=%s created=%s changed=%s failed=%s\n' \
  "$stage" "$successful" "$created" "$changed" "$failed"
