#!/usr/bin/env bash
set -euo pipefail
umask 077

HOST='api.fiducia.cloud'
BASE_URL="https://${HOST}"
EVIDENCE_PATH="${AI_BRIDGE_PUBLIC_EVIDENCE_PATH:-${RUNNER_TEMP:-/tmp}/ai-agent-bridge-public-ingress.json}"
WORK_DIR="$(mktemp -d)"
OBSERVATIONS_PATH="${WORK_DIR}/observations.jsonl"
FAILURES_PATH="${WORK_DIR}/failures.txt"
DNS_PATH="${WORK_DIR}/dns.json"
TLS_LOG_PATH="${WORK_DIR}/tls.log"

cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

: >"${OBSERVATIONS_PATH}"
: >"${FAILURES_PATH}"

record_failure() {
  printf '%s\n' "$1" | tee -a "${FAILURES_PATH}" >&2
}

if ! python3 - "${HOST}" "${DNS_PATH}" <<'PY'
import json
import socket
import sys

host, output_path = sys.argv[1:]
try:
    records = socket.getaddrinfo(host, 443, type=socket.SOCK_STREAM)
    addresses = sorted({record[4][0] for record in records})
    payload = {"host": host, "addresses": addresses, "error": None}
    if not addresses:
        raise RuntimeError("resolver returned no addresses")
except Exception as exc:
    payload = {"host": host, "addresses": [], "error": type(exc).__name__}
    with open(output_path, "w", encoding="utf-8") as handle:
        json.dump(payload, handle, sort_keys=True)
        handle.write("\n")
    raise
else:
    with open(output_path, "w", encoding="utf-8") as handle:
        json.dump(payload, handle, sort_keys=True)
        handle.write("\n")
PY
then
  record_failure 'dns_resolution_failed'
fi

TLS_VERIFIED=false
if timeout 20 openssl s_client \
  -connect "${HOST}:443" \
  -servername "${HOST}" \
  -verify_return_error \
  -verify_hostname "${HOST}" \
  -brief </dev/null >"${TLS_LOG_PATH}" 2>&1; then
  TLS_VERIFIED=true
else
  record_failure 'tls_verification_failed'
fi

record_observation() {
  local name="$1"
  local method="$2"
  local path="$3"
  local expected_status="$4"
  local validator="$5"
  local body_path="${WORK_DIR}/${name}.body"
  local headers_path="${WORK_DIR}/${name}.headers"
  local metadata_path="${WORK_DIR}/${name}.metadata"
  local request_body='probe=unsigned-den-845'
  local curl_exit=0

  local -a curl_args=(
    --silent
    --show-error
    --connect-timeout 10
    --max-time 20
    --max-redirs 0
    --proto '=https'
    --tlsv1.2
    --request "${method}"
    --user-agent 'oresoftware-den-845-public-probe/1'
    --dump-header "${headers_path}"
    --output "${body_path}"
    --write-out $'%{http_code}\n%{remote_ip}\n%{ssl_verify_result}\n%{http_version}\n%{url_effective}\n'
  )

  if [[ "${method}" == 'POST' ]]; then
    curl_args+=(
      --header 'content-type: application/x-www-form-urlencoded'
      --data-binary "${request_body}"
    )
  fi

  set +e
  curl "${curl_args[@]}" "${BASE_URL}${path}" >"${metadata_path}"
  curl_exit=$?
  set -e
  if [[ "${curl_exit}" -ne 0 ]]; then
    record_failure "${name}:transport_failed_exit_${curl_exit}"
  fi

  local status='000'
  local remote_ip=''
  local ssl_verify_result='unknown'
  local http_version='unknown'
  local effective_url=''
  if [[ -s "${metadata_path}" ]]; then
    mapfile -t metadata <"${metadata_path}"
    status="${metadata[0]:-000}"
    remote_ip="${metadata[1]:-}"
    ssl_verify_result="${metadata[2]:-unknown}"
    http_version="${metadata[3]:-unknown}"
    effective_url="${metadata[4]:-}"
  fi

  local body_bytes=0
  local body_sha256=''
  if [[ -f "${body_path}" ]]; then
    body_bytes="$(wc -c <"${body_path}" | tr -d ' ')"
    body_sha256="$(sha256sum "${body_path}" | awk '{print $1}')"
  fi

  local validation_ok=true
  if [[ "${curl_exit}" -ne 0 ]]; then
    validation_ok=false
  fi
  if [[ "${status}" != "${expected_status}" ]]; then
    validation_ok=false
    record_failure "${name}:expected_http_${expected_status}_got_${status}"
  fi
  if [[ "${ssl_verify_result}" != '0' ]]; then
    validation_ok=false
    record_failure "${name}:curl_tls_verification_${ssl_verify_result}"
  fi
  if [[ -z "${remote_ip}" ]]; then
    validation_ok=false
    record_failure "${name}:missing_remote_ip"
  fi
  if [[ "${effective_url}" != "${BASE_URL}${path}" ]]; then
    validation_ok=false
    record_failure "${name}:unexpected_effective_url"
  fi
  if (( body_bytes > 4096 )); then
    validation_ok=false
    record_failure "${name}:response_body_exceeds_4096_bytes"
  fi

  if [[ "${validator}" != 'none' && -f "${body_path}" ]]; then
    if ! python3 - "${validator}" "${body_path}" <<'PY'
import json
import sys

validator, body_path = sys.argv[1:]
with open(body_path, "rb") as handle:
    raw = handle.read()
try:
    payload = json.loads(raw)
except Exception as exc:
    raise SystemExit(f"response is not JSON: {type(exc).__name__}")

if validator == "command_auth_failure":
    assert payload == {
        "response_type": "ephemeral",
        "text": "Request authentication failed.",
    }, payload
elif validator == "empty_object":
    assert payload == {}, payload
else:
    raise SystemExit(f"unknown validator: {validator}")
PY
    then
      validation_ok=false
      record_failure "${name}:response_contract_failed"
    fi
  fi

  python3 - \
    "${OBSERVATIONS_PATH}" \
    "${name}" \
    "${method}" \
    "${path}" \
    "${expected_status}" \
    "${status}" \
    "${remote_ip}" \
    "${ssl_verify_result}" \
    "${http_version}" \
    "${effective_url}" \
    "${body_bytes}" \
    "${body_sha256}" \
    "${validation_ok}" <<'PY'
import json
import sys

(
    output_path,
    name,
    method,
    path,
    expected_status,
    observed_status,
    remote_ip,
    ssl_verify_result,
    http_version,
    effective_url,
    body_bytes,
    body_sha256,
    validation_ok,
) = sys.argv[1:]
record = {
    "name": name,
    "method": method,
    "path": path,
    "expected_status": int(expected_status),
    "observed_status": int(observed_status) if observed_status.isdigit() else observed_status,
    "remote_ip": remote_ip or None,
    "ssl_verify_result": int(ssl_verify_result) if ssl_verify_result.isdigit() else ssl_verify_result,
    "http_version": http_version,
    "effective_url": effective_url,
    "body_bytes": int(body_bytes),
    "body_sha256": body_sha256 or None,
    "response_body_recorded": False,
    "valid": validation_ok == "true",
}
with open(output_path, "a", encoding="utf-8") as handle:
    json.dump(record, handle, sort_keys=True)
    handle.write("\n")
PY
}

record_observation \
  'unsigned_chatgpt_command' \
  'POST' \
  '/slack/commands/ores-chatgpt' \
  '401' \
  'command_auth_failure'
record_observation \
  'unsigned_claude_command' \
  'POST' \
  '/slack/commands/ores-claude' \
  '401' \
  'command_auth_failure'
record_observation \
  'unsigned_interaction' \
  'POST' \
  '/slack/interactions' \
  '401' \
  'empty_object'
record_observation \
  'wrong_method_on_exact_command_route' \
  'GET' \
  '/slack/commands/ores-chatgpt' \
  '405' \
  'none'
record_observation \
  'unknown_slack_route' \
  'GET' \
  '/slack/den-845-public-probe-not-a-route' \
  '404' \
  'none'

mkdir -p "$(dirname "${EVIDENCE_PATH}")"
python3 - \
  "${HOST}" \
  "${DNS_PATH}" \
  "${TLS_VERIFIED}" \
  "${OBSERVATIONS_PATH}" \
  "${FAILURES_PATH}" \
  "${EVIDENCE_PATH}" <<'PY'
import datetime
import json
import sys

host, dns_path, tls_verified, observations_path, failures_path, output_path = sys.argv[1:]
try:
    with open(dns_path, encoding="utf-8") as handle:
        dns = json.load(handle)
except FileNotFoundError:
    dns = {"host": host, "addresses": [], "error": "missing_dns_evidence"}

observations = []
with open(observations_path, encoding="utf-8") as handle:
    for line in handle:
        if line.strip():
            observations.append(json.loads(line))

with open(failures_path, encoding="utf-8") as handle:
    failures = [line.strip() for line in handle if line.strip()]

payload = {
    "schema_version": 1,
    "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "host": host,
    "source_contract": {
        "repository": "ORESoftware/ai-agent-bridge.rs",
        "revision": "ec667946b1f8725b6baea8e67ae6a701d602dc04",
        "command_and_interaction_unsigned_status": 401,
        "exact_command_get_status": 405,
        "unknown_route_status": 404,
    },
    "dns": dns,
    "tls_verified": tls_verified == "true",
    "observations": observations,
    "failures": failures,
    "response_bodies_recorded": False,
    "passed": not failures and bool(dns.get("addresses")) and tls_verified == "true" and all(
        observation.get("valid") for observation in observations
    ),
}
with open(output_path, "w", encoding="utf-8") as handle:
    json.dump(payload, handle, indent=2, sort_keys=True)
    handle.write("\n")
print(json.dumps({
    "host": payload["host"],
    "passed": payload["passed"],
    "dns_address_count": len(dns.get("addresses", [])),
    "tls_verified": payload["tls_verified"],
    "statuses": {
        observation["name"]: observation["observed_status"]
        for observation in observations
    },
    "failures": failures,
}, sort_keys=True))
if not payload["passed"]:
    raise SystemExit(1)
PY
