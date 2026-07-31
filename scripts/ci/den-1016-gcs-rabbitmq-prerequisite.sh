#!/usr/bin/env bash
set -euo pipefail

secret_id='dd/remote-dev/gcs-rabbitmq'
region="${AWS_REGION:-us-east-1}"
payload_file=''
describe_error="$(mktemp /tmp/gcs-rabbitmq-describe.XXXXXX)"

cleanup() {
  if [[ -n "$payload_file" ]]; then
    rm -f "$payload_file"
  fi
  rm -f "$describe_error"
}
trap cleanup EXIT

validate_payload() {
  jq -e '
    type == "object" and
    (.RABBITMQ_DEFAULT_USER | type == "string" and length >= 1) and
    (.RABBITMQ_DEFAULT_PASS | type == "string" and length >= 32) and
    (.RABBITMQ_ERLANG_COOKIE | type == "string" and length >= 32)
  ' >/dev/null
}

if aws secretsmanager describe-secret \
  --region "$region" \
  --secret-id "$secret_id" \
  >/dev/null 2>"$describe_error"; then
  payload="$(
    aws secretsmanager get-secret-value \
      --region "$region" \
      --secret-id "$secret_id" \
      --query SecretString \
      --output text
  )"
  if ! printf '%s' "$payload" | validate_payload; then
    echo 'failure_phase=invalid_existing_secret_schema' >&2
    exit 1
  fi
  secret_state=existing_valid
else
  if ! grep -q 'ResourceNotFoundException' "$describe_error"; then
    echo 'failure_phase=describe_secret_not_authorized' >&2
    exit 1
  fi

  broker_password="$(openssl rand -hex 32)"
  erlang_cookie="$(openssl rand -hex 32)"
  payload="$(
    jq -cn \
      --arg user gcs \
      --arg password "$broker_password" \
      --arg cookie "$erlang_cookie" \
      '{
        RABBITMQ_DEFAULT_USER: $user,
        RABBITMQ_DEFAULT_PASS: $password,
        RABBITMQ_ERLANG_COOKIE: $cookie
      }'
  )"
  payload_file="$(mktemp /tmp/gcs-rabbitmq-secret.XXXXXX.json)"
  chmod 600 "$payload_file"
  printf '%s' "$payload" >"$payload_file"
  aws secretsmanager create-secret \
    --region "$region" \
    --name "$secret_id" \
    --description 'GCS RabbitMQ credentials managed for External Secrets Operator' \
    --secret-string "file://$payload_file" \
    --tags Key=managed-by,Value=k8s-cluster Key=linear-issue,Value=DEN-1016 \
    >/dev/null
  rm -f "$payload_file"
  payload_file=''
  unset broker_password erlang_cookie payload

  payload="$(
    aws secretsmanager get-secret-value \
      --region "$region" \
      --secret-id "$secret_id" \
      --query SecretString \
      --output text
  )"
  if ! printf '%s' "$payload" | validate_payload; then
    echo 'failure_phase=created_secret_schema_mismatch' >&2
    exit 1
  fi
  secret_state=created_and_valid
fi
unset payload
printf 'secret_state=%s\n' "$secret_state"

for attempt in $(seq 1 90); do
  default_ready="$(
    kubectl -n default get externalsecret gcs-rabbitmq \
      -o jsonpath='{range .status.conditions[?(@.type=="Ready")]}{.status}{end}' \
      2>/dev/null || true
  )"
  observability_ready="$(
    kubectl -n observability get externalsecret gcs-rabbitmq-management \
      -o jsonpath='{range .status.conditions[?(@.type=="Ready")]}{.status}{end}' \
      2>/dev/null || true
  )"

  if [[ "$default_ready" == True && "$observability_ready" == True ]]; then
    kubectl -n default get secret gcs-rabbitmq -o json |
      jq -e '.data
        | has("RABBITMQ_DEFAULT_USER") and
          has("RABBITMQ_DEFAULT_PASS") and
          has("RABBITMQ_ERLANG_COOKIE")' >/dev/null
    kubectl -n observability get secret gcs-rabbitmq-management -o json |
      jq -e '.data
        | has("RABBITMQ_MANAGEMENT_USER") and
          has("RABBITMQ_MANAGEMENT_PASSWORD")' >/dev/null
    kubectl -n default rollout status deployment/gcs-rabbitmq --timeout=120s
    kubectl -n observability rollout status deployment/dd-k8s-resource-exporter --timeout=120s
    echo 'external_secret_state=ready'
    echo 'default_external_secret=Ready'
    echo 'observability_external_secret=Ready'
    echo 'affected_workloads=Available'
    exit 0
  fi

  printf 'waiting_for_external_secrets=%s/90\n' "$attempt"
  sleep 10
done

echo 'failure_phase=external_secrets_not_ready' >&2
kubectl -n default get externalsecret gcs-rabbitmq \
  -o custom-columns=NAME:.metadata.name,READY:.status.conditions[-1].status,REASON:.status.conditions[-1].reason \
  --no-headers 2>/dev/null || true
kubectl -n observability get externalsecret gcs-rabbitmq-management \
  -o custom-columns=NAME:.metadata.name,READY:.status.conditions[-1].status,REASON:.status.conditions[-1].reason \
  --no-headers 2>/dev/null || true
exit 1
