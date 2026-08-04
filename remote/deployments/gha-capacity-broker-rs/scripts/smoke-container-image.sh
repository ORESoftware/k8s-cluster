#!/usr/bin/env bash
set -euo pipefail

image="${1:-gha-capacity-broker:test}"
run_suffix="${GITHUB_RUN_ID:-local}-$$"
container="gha-capacity-broker-smoke-${run_suffix}"
secret_dir="$(mktemp -d)"
port=18117

cleanup() {
  set +e
  if docker inspect "$container" >/dev/null 2>&1; then
    echo "--- ${container} logs ---" >&2
    docker logs "$container" >&2 || true
    docker rm -f "$container" >/dev/null 2>&1 || true
  fi
  rm -rf "$secret_dir"
}
trap cleanup EXIT

actual_user="$(docker image inspect "$image" --format '{{.Config.User}}')"
actual_entrypoint="$(docker image inspect "$image" --format '{{json .Config.Entrypoint}}')"
actual_ports="$(docker image inspect "$image" --format '{{json .Config.ExposedPorts}}')"
revision="$(docker image inspect "$image" --format '{{index .Config.Labels "org.opencontainers.image.revision"}}')"
source="$(docker image inspect "$image" --format '{{index .Config.Labels "org.opencontainers.image.source"}}')"

test "$actual_user" = '65532:65532'
test "$actual_entrypoint" = '["/usr/local/bin/gha-capacity-broker"]'
printf '%s' "$actual_ports" | grep -Fq '8117/tcp'
test -n "$revision"
test "$source" = 'https://github.com/ORESoftware/k8s-cluster'

for executable in /usr/bin/cargo /usr/bin/rustc /usr/bin/git /usr/local/bin/cargo /usr/local/bin/rustup; do
  if docker run --rm --entrypoint "$executable" "$image" --version >/dev/null 2>&1; then
    echo "runtime image unexpectedly contains build tooling: ${executable}" >&2
    exit 1
  fi
done

printf '%s' 'not-a-real-private-key-mutation' >"${secret_dir}/mutation.pem"
printf '%s' 'not-a-real-private-key-billing' >"${secret_dir}/billing.pem"
chmod 0755 "$secret_dir"
chmod 0444 "${secret_dir}/mutation.pem" "${secret_dir}/billing.pem"

policy='{"includedMinutes":2000,"warnPercent":75,"selfHostedPercent":90,"hardStopPercent":100,"preferSelfHosted":false,"selfHostedReady":false,"buildServerEnabled":true,"hostedRunsOn":["ubuntu-latest"],"selfHostedRunsOn":["streempilot-ci"],"selectedRepositoryIds":[1294558398]}'

docker run --detach --rm \
  --name "$container" \
  --read-only \
  --tmpfs /tmp:rw,noexec,nosuid,size=16m \
  --security-opt no-new-privileges:true \
  --cap-drop ALL \
  --publish "127.0.0.1:${port}:8117" \
  --volume "${secret_dir}:/var/run/gha-capacity-broker:ro" \
  --env HOST=0.0.0.0 \
  --env PORT=8117 \
  --env RUST_LOG=gha_capacity_broker=info \
  --env GHA_ORGANIZATION=StreemPilot \
  --env "GHA_ORG_POLICY_JSON=${policy}" \
  --env GHA_MUTATION_ENABLED=false \
  --env GHA_RECONCILE_INTERVAL_SECONDS=900 \
  --env SERVER_AUTH_SECRET=capacity-broker-smoke-operator-secret-0001 \
  --env GITHUB_MUTATION_APP_ID=1001 \
  --env GITHUB_MUTATION_APP_INSTALLATION_ID=2001 \
  --env GITHUB_MUTATION_APP_PRIVATE_KEY_PATH=/var/run/gha-capacity-broker/mutation.pem \
  --env GITHUB_BILLING_APP_ID=1002 \
  --env GITHUB_BILLING_APP_INSTALLATION_ID=2002 \
  --env GITHUB_BILLING_APP_PRIVATE_KEY_PATH=/var/run/gha-capacity-broker/billing.pem \
  "$image" >/dev/null

wait_for_json() {
  local endpoint="$1"
  local expected="$2"
  local output=''
  for _ in $(seq 1 60); do
    if output="$(curl --silent --show-error --fail --max-time 2 "http://127.0.0.1:${port}${endpoint}" 2>/dev/null)"; then
      if printf '%s' "$output" | grep -Fq "$expected"; then
        printf '%s\n' "$output"
        return 0
      fi
    fi
    sleep 1
  done
  echo "endpoint never satisfied contract: ${endpoint} expected ${expected}" >&2
  return 1
}

wait_for_json /healthz '"service":"gha-capacity-broker"' >/dev/null
wait_for_json /readyz '"ok":true' >/dev/null
wait_for_json /api/v1/capabilities '"controlPlaneClone":false' >/dev/null
curl --silent --show-error --fail --max-time 2 "http://127.0.0.1:${port}/metrics" \
  | grep -Fq 'gha_capacity_broker_build_info{service="gha-capacity-broker"} 1'

printf '{"ok":true,"image":"%s","mutationEnabled":false,"githubTokenRequested":false}\n' "$image"
