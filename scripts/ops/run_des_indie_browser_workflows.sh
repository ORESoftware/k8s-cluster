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

policy_mode=''
for _ in $(seq 1 240); do
  env_json="$(kubectl -n default get deployment dd-build-server -o json 2>/dev/null || true)"
  if jq -e --arg p "$PLAYWRIGHT_REPO" --arg u "$PUPPETEER_REPO" '
    [.spec.template.spec.containers[] | select(.name == "build-server") | .env[]?]
    | (any(.name == "BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES" and (.value | contains($p) and contains($u))))
      and (any(.name == "BUILD_SERVER_GHA_WORKFLOW_EXECUTION_ENABLED" and .value == "true"))
  ' <<<"$env_json" >/dev/null 2>&1; then
    policy_mode='workflow-engine'
    break
  fi
  if jq -e \
    --arg p_repo "https://github.com/$PLAYWRIGHT_REPO.git" \
    --arg u_repo "https://github.com/$PUPPETEER_REPO.git" '
      [.spec.template.spec.containers[] | select(.name == "build-server") | .env[]?] as $env
      | def envv($name): ([$env[] | select(.name == $name) | .value][0] // "");
        (envv("BUILD_SERVER_GIT_BIN") == "/etc/dd-build-server/git-wrapper.sh") and
        ((envv("BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON") | fromjson? // []) as $rules |
          any($rules[]; .repository == $p_repo and (.profiles | index("playwright") != null)) and
          any($rules[]; .repository == $u_repo and (.profiles | index("puppeteer") != null)))
    ' <<<"$env_json" >/dev/null 2>&1; then
    policy_mode='direct-profile'
    break
  fi
  sleep 3
done
[[ -n "$policy_mode" ]] || {
  echo 'The deployed build server did not reconcile either reviewed DES execution policy.' >&2
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
  if curl --fail --silent --show-error \
    http://127.0.0.1:18100/healthz \
    > "$WORK_DIR/health.json"; then
    ready=true
    break
  fi
  sleep 1
done
[[ "$ready" == true ]]
curl --fail --silent --show-error \
  http://127.0.0.1:18100/readyz \
  > "$WORK_DIR/ready.json"
jq -e '.ok == true and .dependenciesReady == true' "$WORK_DIR/ready.json" >/dev/null

run_one_workflow() {
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
    mode:"workflow-engine",
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

submit_profile() {
  local repository="$1"
  local revision="$2"
  local profile="$3"
  local slug="$4"
  local repo_url="https://github.com/$repository.git"

  jq -nc \
    --arg repoUrl "$repo_url" \
    --arg revision "$revision" \
    --arg profile "$profile" \
    --arg requestId "des-browser-direct-$slug-$revision-$REQUEST_SUFFIX" '
    {
      schemaVersion:"build-server.v1",
      jobKind:"run-profile",
      repoUrl:$repoUrl,
      gitRef:$revision,
      profile:$profile,
      contextDir:".",
      push:false,
      executor:"local",
      requestId:$requestId
    }' > "$WORK_DIR/$slug-request.json"

  curl --fail --silent --show-error \
    --header "x-server-auth: $server_auth" \
    --header 'content-type: application/json' \
    --data-binary "@$WORK_DIR/$slug-request.json" \
    http://127.0.0.1:18100/builds \
    > "$WORK_DIR/$slug-submit.json"
  jq -er \
    --arg repoUrl "$repo_url" \
    --arg revision "$revision" \
    --arg profile "$profile" '
      select(.request.repoUrl == $repoUrl and
             .request.gitRef == $revision and
             .request.profile == $profile and
             .request.jobKind == "run-profile")
      | .id
    ' "$WORK_DIR/$slug-submit.json"
}

wait_profile() {
  local job_id="$1"
  local repository="$2"
  local revision="$3"
  local profile="$4"
  local slug="$5"
  local repo_url="https://github.com/$repository.git"
  local status='queued'

  for _ in $(seq 1 960); do
    if ! curl --fail --silent --show-error \
      --header "x-server-auth: $server_auth" \
      "http://127.0.0.1:18100/builds/$job_id" \
      > "$WORK_DIR/$slug-final.json"; then
      echo "Unable to read indie-worker build $job_id." >&2
      return 1
    fi
    status="$(jq -r '.status' "$WORK_DIR/$slug-final.json")"
    case "$status" in
      succeeded|failed) break ;;
    esac
    sleep 3
  done

  curl --fail --silent --show-error \
    --header "x-server-auth: $server_auth" \
    "http://127.0.0.1:18100/builds/$job_id/logs" \
    > "$WORK_DIR/$slug.log" || true

  if ! jq -e \
    --arg jobId "$job_id" \
    --arg repoUrl "$repo_url" \
    --arg revision "$revision" \
    --arg profile "$profile" '
      .id == $jobId and
      .status == "succeeded" and
      .request.repoUrl == $repoUrl and
      .request.gitRef == $revision and
      .request.profile == $profile and
      .request.jobKind == "run-profile"
    ' "$WORK_DIR/$slug-final.json" >/dev/null; then
    jq -c '{id,status,request,error,createdAtMs,startedAtMs,finishedAtMs}' \
      "$WORK_DIR/$slug-final.json" >&2 || true
    tail -n 160 "$WORK_DIR/$slug.log" >&2 || true
    return 1
  fi

  grep -F -- "--branch $revision" "$WORK_DIR/$slug.log" >/dev/null || {
    echo "Build $job_id did not record the requested immutable revision." >&2
    tail -n 160 "$WORK_DIR/$slug.log" >&2 || true
    return 1
  }

  local log_sha256
  log_sha256="$(sha256sum "$WORK_DIR/$slug.log" | awk '{print $1}')"
  jq -c --arg mode 'direct-profile' --arg logSha256 "$log_sha256" '{
    mode:$mode,
    id,
    requestId:.request.requestId,
    status,
    repository:.request.repoUrl,
    revision:.request.gitRef,
    profile:.request.profile,
    buildId:id,
    logSha256:$logSha256,
    createdAtMs,
    startedAtMs,
    finishedAtMs
  }' "$WORK_DIR/$slug-final.json"
}

echo 'DES_INDIE_EVIDENCE_BEGIN'
echo "{\"policyMode\":\"$policy_mode\"}"
if [[ "$policy_mode" == workflow-engine ]]; then
  curl --fail --silent --show-error \
    --header "x-server-auth: $server_auth" \
    http://127.0.0.1:18100/gha/workflows/capabilities \
    > "$WORK_DIR/capabilities.json"
  jq -e '
    .executionEnabled == true and
    (.profiles | index("playwright") != null) and
    (.profiles | index("puppeteer") != null)
  ' "$WORK_DIR/capabilities.json" >/dev/null

  run_one_workflow "$PLAYWRIGHT_REPO" "$PLAYWRIGHT_SHA" "$PLAYWRIGHT_PATH" playwright "$playwright_yaml" playwright
  run_one_workflow "$PUPPETEER_REPO" "$PUPPETEER_SHA" "$PUPPETEER_PATH" puppeteer "$puppeteer_yaml" puppeteer
else
  jq -e '
    .ok == true and
    .authConfigured == true and
    (.allowedProfiles | index("playwright") != null) and
    (.allowedProfiles | index("puppeteer") != null) and
    (.allowedProfileRepoPrefixes | index("exact-id:discrete-event-systems-test/des-web-playwright-e2e#playwright") != null) and
    (.allowedProfileRepoPrefixes | index("exact-id:discrete-event-systems-test/des-web-puppeteer-e2e#puppeteer") != null)
  ' "$WORK_DIR/health.json" >/dev/null

  playwright_id="$(submit_profile "$PLAYWRIGHT_REPO" "$PLAYWRIGHT_SHA" playwright playwright)"
  puppeteer_id="$(submit_profile "$PUPPETEER_REPO" "$PUPPETEER_SHA" puppeteer puppeteer)"
  [[ "$playwright_id" =~ ^build-[A-Za-z0-9._-]+$ ]]
  [[ "$puppeteer_id" =~ ^build-[A-Za-z0-9._-]+$ ]]

  direct_status=0
  wait_profile "$playwright_id" "$PLAYWRIGHT_REPO" "$PLAYWRIGHT_SHA" playwright playwright || direct_status=1
  wait_profile "$puppeteer_id" "$PUPPETEER_REPO" "$PUPPETEER_SHA" puppeteer puppeteer || direct_status=1
  (( direct_status == 0 ))
fi
echo 'DES_INDIE_EVIDENCE_END'
