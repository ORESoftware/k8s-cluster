#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

: "${ISSUE_TOKEN:?ISSUE_TOKEN is required}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
: "${GITHUB_RUN_ID:?GITHUB_RUN_ID is required}"
: "${TRACKING_ISSUE:?TRACKING_ISSUE is required}"
: "${INSTANCE_ID:?INSTANCE_ID is required}"

readonly TARGET_ORG='discrete-event-systems-test'
readonly EXPECTED_LOGIN='ORESoftware'
readonly PROJECT_TITLE='DES Browser Automation'
readonly API_VERSION='2022-11-28'
readonly HANDOFF_TIMEOUT_SECONDS=840

work="$(mktemp -d "${RUNNER_TEMP:-/tmp}/des-browser-session.XXXXXX")"
private_key="$work/session-private.pem"
public_key="$work/session-public.pem"
public_der="$work/session-public.der"
request_comment_id=''
envelope_comment_id=''
github_token=''
gateway_auth=''

api_request() {
  local method="$1"
  local endpoint="$2"
  local data_file="${3:-}"
  local args=(
    --silent --show-error --fail-with-body
    --request "$method"
    --header 'Accept: application/vnd.github+json'
    --header "Authorization: Bearer ${ISSUE_TOKEN}"
    --header "X-GitHub-Api-Version: ${API_VERSION}"
  )
  if [[ -n "$data_file" ]]; then
    args+=(--header 'Content-Type: application/json' --data-binary "@$data_file")
  fi
  curl "${args[@]}" "https://api.github.com/repos/${GITHUB_REPOSITORY}${endpoint}"
}

delete_comment() {
  local comment_id="${1:-}"
  [[ -n "$comment_id" ]] || return 0
  api_request DELETE "/issues/comments/${comment_id}" >/dev/null 2>&1 || true
}

cleanup() {
  local status=$?
  delete_comment "$envelope_comment_id"
  delete_comment "$request_comment_id"
  unset ISSUE_TOKEN GH_TOKEN GITHUB_TOKEN github_token gateway_auth
  find "$work" -type f -exec sh -c 'for file do : > "$file"; done' sh {} + 2>/dev/null || true
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT INT TERM

for tool in aws base64 curl gh jq openssl sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "Required tool is unavailable: ${tool}" >&2
    exit 69
  }
done

openssl genpkey \
  -algorithm RSA \
  -pkeyopt rsa_keygen_bits:3072 \
  -out "$private_key" >/dev/null 2>&1
chmod 600 "$private_key"
openssl pkey -in "$private_key" -pubout -out "$public_key"
openssl pkey -pubin -in "$public_key" -outform DER -out "$public_der"

nonce="$(openssl rand -hex 24)"
fingerprint="$(sha256sum "$public_der" | awk '{print $1}')"
public_key_b64="$(base64 --wrap=0 "$public_der")"

jq -nc \
  --arg run_id "$GITHUB_RUN_ID" \
  --arg nonce "$nonce" \
  --arg fingerprint "$fingerprint" \
  --arg public_key "$public_key_b64" \
  '{body:(
    "Encrypted one-time credential handoff requested for workflow run `" + $run_id + "`.\n\n" +
    "Only RSA-OAEP ciphertext may be posted in response. The ephemeral private key exists only on the active runner.\n\n" +
    "`pat-public-key-v1:" + $nonce + ":" + $fingerprint + ":" + $public_key + "`"
  )}' > "$work/request-comment.json"
request_comment_id="$(
  api_request POST "/issues/${TRACKING_ISSUE}/comments" "$work/request-comment.json" \
    | jq -er '.id | select(type == "number")'
)"
echo "Published ephemeral public-key handoff request as comment ${request_comment_id}."
unset public_key_b64

prefix="pat-envelope-v1:${nonce}:"
deadline=$((SECONDS + HANDOFF_TIMEOUT_SECONDS))
while (( SECONDS < deadline )); do
  api_request GET "/issues/${TRACKING_ISSUE}/comments?per_page=100" > "$work/comments.json"
  envelope_record="$(
    jq -r --arg prefix "$prefix" '
      [
        .[]
        | select(.user.login == "ORESoftware")
        | select(.body | contains($prefix))
        | {id, body}
      ]
      | last // empty
      | @base64
    ' "$work/comments.json"
  )"
  if [[ -n "$envelope_record" ]]; then
    decoded_record="$(printf '%s' "$envelope_record" | base64 --decode)"
    envelope_comment_id="$(jq -er '.id | select(type == "number")' <<<"$decoded_record")"
    envelope_body="$(jq -er '.body | select(type == "string")' <<<"$decoded_record")"
    ciphertext_b64="${envelope_body#*${prefix}}"
    ciphertext_b64="${ciphertext_b64%%[^A-Za-z0-9+/=]*}"
    if [[ "$ciphertext_b64" =~ ^[A-Za-z0-9+/]+={0,2}$ ]]; then
      break
    fi
    envelope_comment_id=''
  fi
  sleep 3
done

[[ -n "${ciphertext_b64:-}" ]] || {
  echo 'Encrypted credential handoff expired without an envelope.' >&2
  exit 70
}

printf '%s' "$ciphertext_b64" | base64 --decode > "$work/github-token.enc"
openssl pkeyutl \
  -decrypt \
  -inkey "$private_key" \
  -pkeyopt rsa_padding_mode:oaep \
  -pkeyopt rsa_oaep_md:sha256 \
  -in "$work/github-token.enc" \
  -out "$work/github-token.txt"
github_token="$(cat "$work/github-token.txt")"
: > "$work/github-token.txt"
unset ciphertext_b64 envelope_body decoded_record envelope_record

[[ ${#github_token} -ge 20 && "$github_token" != *[[:space:]]* ]] || {
  echo 'The decrypted GitHub credential failed structural validation.' >&2
  exit 71
}
echo "::add-mask::$github_token"
export GH_TOKEN="$github_token" GITHUB_TOKEN="$github_token" GH_HOST=github.com

delete_comment "$envelope_comment_id"
envelope_comment_id=''
delete_comment "$request_comment_id"
request_comment_id=''

actor="$(gh api user --jq .login)"
[[ "$actor" == "$EXPECTED_LOGIN" ]] || {
  echo "Unexpected GitHub identity: ${actor}" >&2
  exit 72
}
membership="$(gh api "user/memberships/orgs/${TARGET_ORG}" --jq '[.state,.role] | join(":")')"
[[ "$membership" == 'active:admin' ]] || {
  echo "${actor} lacks active admin membership in ${TARGET_ORG}." >&2
  exit 73
}

ensure_repo() {
  local name="$1"
  local description="$2"
  local full="${TARGET_ORG}/${name}"
  if ! gh repo view "$full" >/dev/null 2>&1; then
    gh repo create "$full" --public --description "$description" --add-readme
  fi
  gh api -X PATCH "repos/${full}" \
    -f has_issues=true \
    -f has_projects=true \
    -f has_wiki=false \
    -f delete_branch_on_merge=true >/dev/null
  printf 'REPOSITORY_READY %s\n' "$full"
}

ensure_repo des-web-playwright-e2e \
  'Playwright browser contracts for des-web across GitHub Actions and gha-indie-worker.'
ensure_repo des-web-puppeteer-e2e \
  'Puppeteer browser contracts for des-web across GitHub Actions and gha-indie-worker.'
ensure_repo .github \
  'Organization profile and DES browser-test fleet documentation.'

ensure_project() {
  local owner="$1"
  local number=''
  number="$(
    gh project list --owner "$owner" --format json \
      --jq ".projects[] | select(.title == \"${PROJECT_TITLE}\") | .number" \
      2>/dev/null | head -1 || true
  )"
  if [[ -z "$number" ]]; then
    number="$(
      gh project create --owner "$owner" --title "$PROJECT_TITLE" \
        --format json --jq .number
    )"
  fi
  printf 'PROJECT_READY %s https://github.com/orgs/%s/projects/%s\n' "$owner" "$owner" "$number"
}

ensure_project discrete-event-systems
ensure_project discrete-event-systems-test

# Retrieve the gateway credential from the protected cluster without returning
# plaintext through SSM. The host encrypts it to this runner's ephemeral key.
public_key_b64="$(base64 --wrap=0 "$public_der")"
remote_script="$work/gateway-envelope.sh"
cat > "$remote_script" <<REMOTE_SCRIPT
#!/usr/bin/env bash
set -Eeuo pipefail
umask 077
nonce='$nonce'
public_key_b64='$public_key_b64'
work=\$(mktemp -d /tmp/des-gateway-envelope.XXXXXX)
cleanup() {
  unset value encoded secret_json
  find "\$work" -type f -exec sh -c 'for file do : > "\$file"; done' sh {} + 2>/dev/null || true
  rm -rf "\$work"
}
trap cleanup EXIT
printf '%s' "\$public_key_b64" | base64 --decode > "\$work/public.der"
openssl pkey -pubin -inform DER -in "\$work/public.der" -out "\$work/public.pem"
value=''
for kubeconfig in /etc/kubernetes/admin.conf /root/.kube/config /home/ec2-user/.kube/config; do
  [[ -r "\$kubeconfig" ]] || continue
  encoded=\$(KUBECONFIG="\$kubeconfig" kubectl -n default get secret dd-remote-auth-secrets -o jsonpath='{.data.DD_AUTH_COOKIE_VALUE}' 2>/dev/null || true)
  [[ -n "\$encoded" ]] || continue
  value=\$(printf '%s' "\$encoded" | base64 --decode 2>/dev/null || true)
  [[ -n "\$value" ]] && break
 done
if [[ -z "\$value" ]] && command -v aws >/dev/null 2>&1; then
  for secret_id in dd/remote-dev/auth-secrets dd/remote-dev/remote-auth-secrets dd/remote-dev/gateway-auth-secrets; do
    secret_json=\$(aws secretsmanager get-secret-value --region "\${AWS_REGION:-\${AWS_DEFAULT_REGION:-us-east-1}}" --secret-id "\$secret_id" --query SecretString --output text 2>/dev/null || true)
    [[ -n "\$secret_json" ]] || continue
    value=\$(SECRET_KEY=DD_AUTH_COOKIE_VALUE python3 -c 'import json,os,sys; p=json.load(sys.stdin); v=p.get(os.environ["SECRET_KEY"]); sys.stdout.write(v if isinstance(v,str) else "")' <<<"\$secret_json" 2>/dev/null || true)
    [[ -n "\$value" ]] && break
  done
fi
[[ \${#value} -ge 20 && "\$value" != *[[:space:]]* ]]
printf '%s' "\$value" > "\$work/value.txt"
openssl pkeyutl -encrypt -pubin -inkey "\$work/public.pem" -pkeyopt rsa_padding_mode:oaep -pkeyopt rsa_oaep_md:sha256 -in "\$work/value.txt" -out "\$work/value.enc"
: > "\$work/value.txt"
printf 'gateway-envelope-v1:%s:%s\n' "\$nonce" "\$(base64 --wrap=0 "\$work/value.enc")"
REMOTE_SCRIPT
chmod 700 "$remote_script"
encoded_remote_script="$(base64 --wrap=0 "$remote_script")"
remote_command="printf '%s' '$encoded_remote_script' | base64 --decode > /tmp/des-gateway-envelope.sh; chmod 700 /tmp/des-gateway-envelope.sh; set +e; /tmp/des-gateway-envelope.sh; status=\$?; rm -f /tmp/des-gateway-envelope.sh; exit \$status"
parameters="$work/ssm-parameters.json"
jq -nc --arg command "$remote_command" '{commands:[$command]}' > "$parameters"
command_id="$(
  aws ssm send-command \
    --instance-ids "$INSTANCE_ID" \
    --document-name AWS-RunShellScript \
    --comment "DES gateway secret envelope for workflow ${GITHUB_RUN_ID}" \
    --parameters "file://$parameters" \
    --query 'Command.CommandId' \
    --output text
)"
status=Pending
for _ in $(seq 1 240); do
  status="$(
    aws ssm get-command-invocation \
      --command-id "$command_id" \
      --instance-id "$INSTANCE_ID" \
      --query Status \
      --output text 2>/dev/null || true
  )"
  case "$status" in Success|Failed|Cancelled|TimedOut|Cancelling) break ;; esac
  sleep 2
done
invocation="$work/ssm-invocation.json"
aws ssm get-command-invocation \
  --command-id "$command_id" \
  --instance-id "$INSTANCE_ID" \
  --output json > "$invocation"
[[ "$(jq -r '.Status' "$invocation")" == 'Success' ]] || {
  echo 'Protected gateway credential envelope failed.' >&2
  jq -r '.StandardErrorContent' "$invocation" >&2
  exit 74
}
gateway_prefix="gateway-envelope-v1:${nonce}:"
gateway_ciphertext="$(
  jq -r '.StandardOutputContent' "$invocation" \
    | sed -n "s#^${gateway_prefix}##p" \
    | tail -1
)"
[[ "$gateway_ciphertext" =~ ^[A-Za-z0-9+/]+={0,2}$ ]] || {
  echo 'Protected gateway envelope was malformed.' >&2
  exit 75
}
printf '%s' "$gateway_ciphertext" | base64 --decode > "$work/gateway.enc"
openssl pkeyutl \
  -decrypt \
  -inkey "$private_key" \
  -pkeyopt rsa_padding_mode:oaep \
  -pkeyopt rsa_oaep_md:sha256 \
  -in "$work/gateway.enc" \
  -out "$work/gateway.txt"
gateway_auth="$(cat "$work/gateway.txt")"
: > "$work/gateway.txt"
unset gateway_ciphertext public_key_b64 encoded_remote_script remote_command
[[ ${#gateway_auth} -ge 20 && "$gateway_auth" != *[[:space:]]* ]] || {
  echo 'Gateway credential failed structural validation.' >&2
  exit 76
}
echo "::add-mask::$gateway_auth"

for repo in des-web-playwright-e2e des-web-puppeteer-e2e; do
  gh secret set DES_GATEWAY_AUTH \
    --repo "${TARGET_ORG}/${repo}" \
    --body "$gateway_auth"
  printf 'SECRET_READY %s/%s DES_GATEWAY_AUTH\n' "$TARGET_ORG" "$repo"
done

jq -nc \
  --arg actor "$actor" \
  '{body:(
    "Encrypted session publication completed.\n\n" +
    "- Authenticated actor: `" + $actor + "`\n" +
    "- Repositories created or verified: **3**\n" +
    "- Organization projects created or verified: **2**\n" +
    "- Gateway Actions secrets configured: **2**\n\n" +
    "The ephemeral private key, plaintext credentials, and handoff comments were removed."
  )}' > "$work/success-comment.json"
api_request POST "/issues/${TRACKING_ISSUE}/comments" "$work/success-comment.json" >/dev/null

printf 'DES_BROWSER_SESSION_PUBLICATION_COMPLETE actor=%s\n' "$actor"
