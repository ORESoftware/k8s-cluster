#!/usr/bin/env bash
set -Eeuo pipefail

: "${INSTANCE_ID:?INSTANCE_ID is required}"

playwright_yaml=$(cat <<'PLAYWRIGHT_YAML'
name: gha-indie-worker Playwright continuity
on:
  workflow_dispatch:
jobs:
  browser-contracts:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
      - uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020
        with:
          node-version: "22"
      - run: npm ci
      - run: npx playwright test
PLAYWRIGHT_YAML
)
puppeteer_yaml=$(cat <<'PUPPETEER_YAML'
name: gha-indie-worker Puppeteer continuity
on:
  workflow_dispatch:
jobs:
  browser-contracts:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
      - uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020
        with:
          node-version: "22"
      - run: npm ci
      - run: npm run test:puppeteer
PUPPETEER_YAML
)

remote_script="${RUNNER_TEMP:?RUNNER_TEMP is required}/verify-des-indie-plans.sh"
cat > "$remote_script" <<REMOTE_SCRIPT
#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly PLAYWRIGHT_REPO='discrete-event-systems-test/des-web-playwright-e2e'
readonly PLAYWRIGHT_URL='https://github.com/discrete-event-systems-test/des-web-playwright-e2e.git'
readonly PLAYWRIGHT_SHA='781e6bc615e3e8fa27e0f3ad5530fb16e93d5719'
readonly PLAYWRIGHT_YAML_B64='$(printf '%s' "$playwright_yaml" | base64 --wrap=0)'
readonly PUPPETEER_REPO='discrete-event-systems-test/des-web-puppeteer-e2e'
readonly PUPPETEER_URL='https://github.com/discrete-event-systems-test/des-web-puppeteer-e2e.git'
readonly PUPPETEER_SHA='7de6de09ea957ae34872d957cc439743c22e8d9b'
readonly PUPPETEER_YAML_B64='$(printf '%s' "$puppeteer_yaml" | base64 --wrap=0)'

kubeconfig=''
for candidate in /etc/kubernetes/admin.conf /root/.kube/config /home/ec2-user/.kube/config; do
  if [[ -r "\$candidate" ]]; then
    kubeconfig="\$candidate"
    break
  fi
done
test -n "\$kubeconfig"
export KUBECONFIG="\$kubeconfig"

cleanup() {
  unset server_auth encoded_auth
  if [[ -n "\${port_forward_pid:-}" ]]; then
    kill "\$port_forward_pid" 2>/dev/null || true
    wait "\$port_forward_pid" 2>/dev/null || true
  fi
  rm -rf /tmp/des-indie-plan-verification
}
trap cleanup EXIT
mkdir -p /tmp/des-indie-plan-verification

policy_ready=false
for _ in \$(seq 1 240); do
  env_json="\$(kubectl -n default get deployment dd-build-server -o json 2>/dev/null || true)"
  if jq -e \
    --arg playwright_url "\$PLAYWRIGHT_URL" \
    --arg puppeteer_url "\$PUPPETEER_URL" \
    --arg playwright_repo "\$PLAYWRIGHT_REPO" \
    --arg puppeteer_repo "\$PUPPETEER_REPO" '
      [.spec.template.spec.containers[]? | select(.name == "build-server") | .env[]?] as \$env
      | ((\$env | map(select(.name == "BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON")) | first | .value // "[]") | (try fromjson catch [])) as \$rules
      | ((\$env | map(select(.name == "BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES")) | first | .value // "") as \$prefixes
      | (
          ((\$rules | any(.repository == \$playwright_url and .profiles == ["playwright"])) and
           (\$rules | any(.repository == \$puppeteer_url and .profiles == ["puppeteer"])))
          or
          ((\$prefixes | contains(\$playwright_repo)) and
           (\$prefixes | contains(\$puppeteer_repo)))
        )
    ' <<<"\$env_json" >/dev/null 2>&1; then
    policy_ready=true
    break
  fi
  sleep 3
done
test "\$policy_ready" = true
kubectl -n default rollout status deployment/dd-build-server --timeout=300s >/dev/null

encoded_auth="\$(kubectl -n default get secret dd-agent-secrets -o jsonpath='{.data.SERVER_AUTH_SECRET}')"
test -n "\$encoded_auth"
server_auth="\$(printf '%s' "\$encoded_auth" | base64 --decode)"
unset encoded_auth
test -n "\$server_auth"

kubectl -n default port-forward service/dd-build-server 18100:8100 --address 127.0.0.1 \
  > /tmp/des-indie-plan-verification/port-forward.log 2>&1 &
port_forward_pid=\$!
for _ in \$(seq 1 60); do
  if curl --fail --silent --header "x-server-auth: \$server_auth" \
    http://127.0.0.1:18100/gha/workflows/capabilities \
    > /tmp/des-indie-plan-verification/capabilities.json; then
    break
  fi
  sleep 1
done
jq -e '
  .executionEnabled == false and
  (.profiles | index("playwright") != null) and
  (.profiles | index("puppeteer") != null)
' /tmp/des-indie-plan-verification/capabilities.json >/dev/null

plan_one() {
  local repository="\$1"
  local revision="\$2"
  local expected_profile="\$3"
  local yaml_b64="\$4"
  local slug="\$5"
  local workflow_yaml
  workflow_yaml="\$(printf '%s' "\$yaml_b64" | base64 --decode)"
  jq -nc \
    --arg repository "\$repository" \
    --arg revision "\$revision" \
    --arg workflowYaml "\$workflow_yaml" \
    --arg requestId "des-browser-plan-\$slug-\$revision" \
    '{
      schemaVersion:"gha-indie-workflow.v1",
      repository:\$repository,
      revision:\$revision,
      workflowPath:".github/workflows/gha-indie-worker.yml",
      workflowYaml:\$workflowYaml,
      requestId:\$requestId
    }' > "/tmp/des-indie-plan-verification/\$slug-request.json"
  curl --fail --silent --show-error \
    --header "x-server-auth: \$server_auth" \
    --header 'content-type: application/json' \
    --data-binary "@/tmp/des-indie-plan-verification/\$slug-request.json" \
    http://127.0.0.1:18100/gha/workflows/plan \
    > "/tmp/des-indie-plan-verification/\$slug-plan.json"
  jq -e --arg repository "\$repository" --arg revision "\$revision" --arg profile "\$expected_profile" '
    .schemaVersion == "gha-indie-plan.v1" and
    .repository == \$repository and
    .revision == \$revision and
    .immutableRevision == true and
    .executable == true and
    (.jobs | length == 1) and
    .jobs[0].supported == true and
    .jobs[0].profile == \$profile
  ' "/tmp/des-indie-plan-verification/\$slug-plan.json" >/dev/null
  jq -c '{
    repository,
    revision,
    immutableRevision,
    executable,
    profile:.jobs[0].profile,
    executionEnabled:false
  }' "/tmp/des-indie-plan-verification/\$slug-plan.json"
}

plan_one "\$PLAYWRIGHT_REPO" "\$PLAYWRIGHT_SHA" playwright "\$PLAYWRIGHT_YAML_B64" playwright
plan_one "\$PUPPETEER_REPO" "\$PUPPETEER_SHA" puppeteer "\$PUPPETEER_YAML_B64" puppeteer
REMOTE_SCRIPT
chmod 700 "$remote_script"

encoded_script="$(base64 --wrap=0 "$remote_script")"
remote_command="printf '%s' '$encoded_script' | base64 --decode > /tmp/verify-des-indie-plans.sh; chmod 700 /tmp/verify-des-indie-plans.sh; set +e; /tmp/verify-des-indie-plans.sh > /tmp/verify-des-indie-plans.log 2>&1; status=\$?; set -e; tail -c 20000 /tmp/verify-des-indie-plans.log 2>/dev/null || true; rm -f /tmp/verify-des-indie-plans.sh /tmp/verify-des-indie-plans.log; exit \$status"
parameters="$RUNNER_TEMP/ssm-parameters.json"
jq -nc --arg command "$remote_command" '{commands:[$command]}' > "$parameters"

command_id="$(aws ssm send-command \
  --instance-ids "$INSTANCE_ID" \
  --document-name AWS-RunShellScript \
  --comment "Verify immutable DES browser plans through gha-indie-worker" \
  --parameters "file://$parameters" \
  --query 'Command.CommandId' \
  --output text)"
echo "ssm_command_id=$command_id"

status=Pending
for _ in $(seq 1 480); do
  status="$(aws ssm get-command-invocation \
    --command-id "$command_id" \
    --instance-id "$INSTANCE_ID" \
    --query Status \
    --output text 2>/dev/null || true)"
  case "$status" in Success|Failed|Cancelled|TimedOut|Cancelling) break ;; esac
  sleep 3
done
aws ssm get-command-invocation \
  --command-id "$command_id" \
  --instance-id "$INSTANCE_ID" \
  --query '{Status:Status,Stdout:StandardOutputContent,Stderr:StandardErrorContent}' \
  --output json
test "$status" = Success
