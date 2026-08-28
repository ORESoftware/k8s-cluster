#!/usr/bin/env bash
set -euo pipefail

clone_image="${1:-gha-clone-server:test}"
router_image="${2:-gha-executor-router:test}"
run_suffix="${GITHUB_RUN_ID:-local}-$$"
clone_name="gha-clone-smoke-${run_suffix}"
router_name="gha-router-smoke-${run_suffix}"
secret_dir="$(mktemp -d)"
clone_port=18125
router_port=18126

cleanup() {
  set +e
  for container in "$clone_name" "$router_name"; do
    if docker inspect "$container" >/dev/null 2>&1; then
      echo "--- ${container} logs ---" >&2
      docker logs "$container" >&2 || true
      docker rm -f "$container" >/dev/null 2>&1 || true
    fi
  done
  rm -rf "$secret_dir"
}
trap cleanup EXIT

assert_image_contract() {
  local image="$1"
  local expected_entrypoint="$2"
  local expected_port="$3"
  local actual_user actual_entrypoint actual_ports revision source

  actual_user="$(docker image inspect "$image" --format '{{.Config.User}}')"
  actual_entrypoint="$(docker image inspect "$image" --format '{{json .Config.Entrypoint}}')"
  actual_ports="$(docker image inspect "$image" --format '{{json .Config.ExposedPorts}}')"
  revision="$(docker image inspect "$image" --format '{{index .Config.Labels "org.opencontainers.image.revision"}}')"
  source="$(docker image inspect "$image" --format '{{index .Config.Labels "org.opencontainers.image.source"}}')"

  test "$actual_user" = '65532:65532'
  test "$actual_entrypoint" = "[\"${expected_entrypoint}\"]"
  printf '%s' "$actual_ports" | grep -Fq "${expected_port}/tcp"
  test -n "$revision"
  test "$source" = 'https://github.com/ORESoftware/k8s-cluster'
}

wait_for_json_endpoint() {
  local url="$1"
  local expected_fragment="$2"
  local output=''
  for _ in $(seq 1 60); do
    if output="$(curl --silent --show-error --fail --max-time 2 "$url" 2>/dev/null)"; then
      if printf '%s' "$output" | grep -Fq "$expected_fragment"; then
        printf '%s\n' "$output"
        return 0
      fi
    fi
    sleep 1
  done
  echo "endpoint never satisfied contract: ${url} expected ${expected_fragment}" >&2
  return 1
}

assert_image_contract "$clone_image" '/usr/local/bin/gha-clone-server' '8125'
assert_image_contract "$router_image" '/usr/local/bin/gha-executor-router' '8126'

# Prove the image defaults do not expose the build toolchain.
for image in "$clone_image" "$router_image"; do
  if docker run --rm --entrypoint /usr/bin/cargo "$image" --version >/dev/null 2>&1; then
    echo "runtime image unexpectedly contains cargo: ${image}" >&2
    exit 1
  fi
  if docker run --rm --entrypoint /usr/bin/git "$image" --version >/dev/null 2>&1; then
    echo "runtime image unexpectedly contains git: ${image}" >&2
    exit 1
  fi
done

docker run --detach --rm \
  --name "$clone_name" \
  --read-only \
  --tmpfs /tmp:rw,noexec,nosuid,size=16m \
  --security-opt no-new-privileges:true \
  --cap-drop ALL \
  --publish "127.0.0.1:${clone_port}:8125" \
  --env HOST=0.0.0.0 \
  --env PORT=8125 \
  --env RUST_LOG=gha_clone_server=info \
  --env GHA_CLONE_AUTH_SECRET=clone-smoke-auth-secret-value-0001 \
  --env GHA_CLONE_GITHUB_WEBHOOK_SECRET=clone-smoke-webhook-secret-value-0001 \
  --env GHA_CLONE_GITHUB_TOKEN=clone-smoke-installation-token-value-0001 \
  --env GHA_CLONE_GITHUB_API_BASE_URL=https://api.github.com \
  --env GHA_CLONE_ALLOWED_REPOSITORIES=ORESoftware/k8s-cluster \
  --env 'GHA_CLONE_WORKFLOW_RULES_JSON={"ORESoftware/k8s-cluster":[".github/workflows/gha-clone-server-meta.yml"]}' \
  --env GHA_CLONE_BUILD_SERVER_URL=http://127.0.0.1:19999 \
  --env GHA_CLONE_BUILD_SERVER_AUTH=router-smoke-inbound-auth-value-0001 \
  --env GHA_CLONE_EXECUTION_ENABLED=false \
  --env GHA_CLONE_WEBHOOK_EXECUTION_ENABLED=false \
  --env GHA_CLONE_WEBHOOK_FAILURE_CONCLUSIONS=failure \
  --env 'GHA_CLONE_WEBHOOK_IGNORED_WORKFLOWS=GHA continuity server' \
  "$clone_image" >/dev/null

wait_for_json_endpoint "http://127.0.0.1:${clone_port}/healthz" '"ok":true' >/dev/null
wait_for_json_endpoint "http://127.0.0.1:${clone_port}/readyz" '"ok":true' >/dev/null

printf '%s' 'router-smoke-inbound-auth-value-0001' >"${secret_dir}/inbound_auth"
printf '%s' 'router-smoke-aws-auth-value-00000001' >"${secret_dir}/aws_auth"
chmod 0755 "$secret_dir"
chmod 0444 "${secret_dir}/inbound_auth" "${secret_dir}/aws_auth"

router_routes='[{"id":"aws-primary","provider":"aws","enabled":true,"url":"http://127.0.0.1:19999","authPath":"/var/run/secrets/gha-executor-router/aws_auth"},{"id":"hetzner-secondary","provider":"hetzner","enabled":false}]'

docker run --detach --rm \
  --name "$router_name" \
  --read-only \
  --tmpfs /tmp:rw,noexec,nosuid,size=16m \
  --security-opt no-new-privileges:true \
  --cap-drop ALL \
  --publish "127.0.0.1:${router_port}:8126" \
  --volume "${secret_dir}:/var/run/secrets/gha-executor-router:ro" \
  --env HOST=0.0.0.0 \
  --env PORT=8126 \
  --env RUST_LOG=gha_executor_router=info \
  --env GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED=false \
  --env GHA_EXECUTOR_ROUTER_AUTH_PATH=/var/run/secrets/gha-executor-router/inbound_auth \
  --env "GHA_EXECUTOR_ROUTER_EXECUTORS_JSON=${router_routes}" \
  "$router_image" >/dev/null

wait_for_json_endpoint "http://127.0.0.1:${router_port}/healthz" '"ok":true' >/dev/null
wait_for_json_endpoint "http://127.0.0.1:${router_port}/readyz" '"executionReady":true' >/dev/null

printf '{"ok":true,"cloneImage":"%s","routerImage":"%s"}\n' "$clone_image" "$router_image"
