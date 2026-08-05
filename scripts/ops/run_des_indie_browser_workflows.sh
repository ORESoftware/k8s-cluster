#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly PLAYWRIGHT_REPO='discrete-event-systems-test/des-web-playwright-e2e'
readonly PLAYWRIGHT_SHA='1e1116ef6811c4e3e6be34ad3e1def39bc20ef59'
readonly PLAYWRIGHT_PATH='.gha/workflows/playwright.yml'
readonly PUPPETEER_REPO='discrete-event-systems-test/des-web-puppeteer-e2e'
readonly PUPPETEER_SHA='0547548429d937023a124de37afca7659a85c3dd'
readonly PUPPETEER_PATH='.gha/workflows/puppeteer.yml'
readonly REQUEST_SUFFIX="${DES_REQUEST_SUFFIX:?DES_REQUEST_SUFFIX is required}"
readonly WORK_DIR='/tmp/des-indie-browser-execution'

playwright_yaml=$(cat <<'PLAYWRIGHT_YAML'
name: DES Playwright continuity
on: [workflow_dispatch]
jobs:
  browser:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
      - uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020
        with:
          node-version: '22'
      - run: npm ci
      - run: npx playwright test
PLAYWRIGHT_YAML
)

puppeteer_yaml=$(cat <<'PUPPETEER_YAML'
name: DES Puppeteer continuity
on: [workflow_dispatch]
jobs:
  browser:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
      - uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020
        with:
          node-version: '22'
      - run: npm ci
      - run: npm run test:puppeteer
PUPPETEER_YAML
)

kubeconfig=''
for candidate in /etc/kubernetes/admin.conf /root/.kube/config /home/ec2-user/.kube/config; do
  if [[ -r "$candidate" ]]; then
    kubeconfig="$candidate"
    break
  fi
done
[[ -n "$kubeconfig" ]] || {
  echo 'No readable Kubernetes administrator configuration was found.' >&2
  exit 69
}
export KUBECONFIG="$kubeconfig"

server_auth=''
port_forward_pid=''
cleanup() {
  unset server_auth encoded_auth
  if [[ -n "$port_forward_pid" ]]; then
    kill "$port_forward_pid" 2>/dev/null || true
    wait "$port_forward_pid" 2>/dev/null || true
  fi
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT INT TERM
rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR"

policy_ready=false
for _ in $(seq 1 240); do
  env_json="$(kubectl -n default get deployment dd-build-server -o json 2>/dev/null || true)"
  if jq -e --arg p "$PLAYWRIGHT_REPO" --arg u "$PUPPETEER_REPO" '
    [.spec.template.spec.containers[] | select(.name == "build-server") | .env[]?]
    | (any(.name == "BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES" and (.value | contains($p) and contains($u))))
      and (any(.name == "BUILD_SERVER_GHA_WORKFLOW_EXECUTION_ENABLED" and .value == "true"))
  ' <<<"$env_json" >/dev/null 2>&1; then
    policy_ready=true
    break
  fi
  sleep 3
done
[[ "$policy_ready" == true ]] || {
  echo 'The deployed build server did not reconcile the DES execution policy.' >&2
  exit 70
}
kubectl -n default rollout status deployment/dd-build-server --timeout=600s >/dev/null

encoded_auth="$(kubectl -n default get secret dd-agent-secrets -o jsonpath='{.data.SERVER_AUTH_SECRET}')"
[[ -n "$encoded_auth" ]]
server_auth="$(printf '%s' "$encoded_auth" | base64 --decode)"
unset encoded_auth
[[ -n "$server_auth" ]]

kubectl -n default port-forward service/dd-build-server 18100:8100 --address 127.0.0.1 \
  > "$WORK_DIR/port-forward.log" 2>&1 &
port_forward_pid=$!
ready=false
for _ in $(seq 1 90); do
  if curl --fail --silent --header "x-server-auth: $server_auth" \
    http://127.0.0.1:18100/gha/workflows/capabilities \
    > "$WORK_DIR/capabilities.json"; then
    ready=true
    break
  fi
  sleep 1
done
[[ "$ready" == true ]]
jq -e '
  .executionEnabled == true and
  (.profiles | index("playwright") != null) and
  (.profiles | index("puppeteer") != null)
' "$WORK_DIR/capabilities.json" >/dev/null

run_one() {
  local repository="$1"
  local revision="$2"
  local workflow_path="$3"
  local expected_profile="$4"
  local workflow_yaml="$5"
  local slug="$6"
  local run_id status

  jq -nc \
    --arg repository "$repository" \
    --arg revision "$revision" \
    --arg workflowPath "$workflow_path" \
    --arg workflowYaml "$workflow_yaml" \
    --arg requestId "des-browser-$slug-$revision-$REQUEST_SUFFIX" \
    '{
      schemaVersion:"gha-indie-workflow.v1",
      repository:$repository,
      revision:$revision,
      workflowPath:$workflowPath,
      workflowYaml:$workflowYaml,
      requestId:$requestId
    }' > "$WORK_DIR/$slug-request.json"

  curl --fail --silent --show-error \
    --header "x-server-auth: $server_auth" \
    --header 'content-type: application/json' \
    --data-binary "@$WORK_DIR/$slug-request.json" \
    http://127.0.0.1:18100/gha/workflows/plan \
    > "$WORK_DIR/$slug-plan.json"
  jq -e --arg repository "$repository" --arg revision "$revision" --arg profile "$expected_profile" '
    .schemaVersion == "gha-indie-plan.v1" and
    .repository == $repository and
    .revision == $revision and
    .immutableRevision == true and
    .executable == true and
    (.jobs | length == 1) and
    .jobs[0].supported == true and
    .jobs[0].profile == $profile
  ' "$WORK_DIR/$slug-plan.json" >/dev/null

  curl --fail --silent --show-error \
    --header "x-server-auth: $server_auth" \
    --header 'content-type: application/json' \
    --data-binary "@$WORK_DIR/$slug-request.json" \
    http://127.0.0.1:18100/gha/workflows/runs \
    > "$WORK_DIR/$slug-run.json"
  run_id="$(jq -er '.id' "$WORK_DIR/$slug-run.json")"
  [[ "$run_id" =~ ^gha-[A-Za-z0-9._-]+$ ]]

  status=queued
  for _ in $(seq 1 960); do
    curl --fail --silent --show-error \
      --header "x-server-auth: $server_auth" \
      "http://127.0.0.1:18100/gha/workflows/runs/$run_id" \
      > "$WORK_DIR/$slug-final.json"
    status="$(jq -r '.status' "$WORK_DIR/$slug-final.json")"
    case "$status" in
      succeeded|failed) break ;;
    esac
    sleep 3
  done

  jq -e --arg repository "$repository" --arg revision "$revision" '
    .status == "succeeded" and
    .plan.repository == $repository and
    .plan.revision == $revision and
    (.jobs | length == 1) and
    (all(.jobs[]; .status == "succeeded" and (.buildId | type == "string" and length > 0)))
  ' "$WORK_DIR/$slug-final.json" >/dev/null
  jq -c '{
    id,
    requestId,
    status,
    repository:.plan.repository,
    revision:.plan.revision,
    profile:.jobs[0].profile,
    buildId:.jobs[0].buildId,
    createdAtMs,
    startedAtMs,
    finishedAtMs
  }' "$WORK_DIR/$slug-final.json"
}

echo 'DES_INDIE_EVIDENCE_BEGIN'
run_one "$PLAYWRIGHT_REPO" "$PLAYWRIGHT_SHA" "$PLAYWRIGHT_PATH" playwright "$playwright_yaml" playwright
run_one "$PUPPETEER_REPO" "$PUPPETEER_SHA" "$PUPPETEER_PATH" puppeteer "$puppeteer_yaml" puppeteer
echo 'DES_INDIE_EVIDENCE_END'
