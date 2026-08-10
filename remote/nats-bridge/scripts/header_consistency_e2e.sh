#!/usr/bin/env bash
        set -euo pipefail

        NATS_CONTAINER="nats-header-consistency-e2e"
        NATS_PORT="${NATS_HEADER_TEST_PORT:-15222}"
        MON_PORT="${NATS_HEADER_MONITOR_PORT:-19222}"
        BRIDGE_PORT="${NATS_HEADER_BRIDGE_PORT:-14004}"
        BRIDGE_TOKEN_VALUE="nats-header-test-token-0123456789"
        BRIDGE_URL="http://127.0.0.1:${BRIDGE_PORT}"
        ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
        BRIDGE_DIR="${ROOT}/remote/nats-bridge"
        WORK="$(mktemp -d)"
        BRIDGE_PID=""

        cleanup() {
          [[ -z "$BRIDGE_PID" ]] || kill "$BRIDGE_PID" >/dev/null 2>&1 || true
          docker rm -f "$NATS_CONTAINER" >/dev/null 2>&1 || true
          rm -rf "$WORK"
        }
        trap cleanup EXIT

        wait_for() {
          local deadline=$((SECONDS + $1)); shift
          while (( SECONDS < deadline )); do
            "$@" >/dev/null 2>&1 && return 0
            sleep 1
          done
          return 1
        }

        status() {
          curl --silent --output /dev/null --write-out '%{http_code}' "$@" || true
        }

        docker rm -f "$NATS_CONTAINER" >/dev/null 2>&1 || true
        docker run --detach --name "$NATS_CONTAINER" \
          --publish "${NATS_PORT}:4222" --publish "${MON_PORT}:8222" \
          nats:2.11.17-alpine -js -m 8222 >/dev/null
        wait_for 30 curl --fail --silent "http://127.0.0.1:${MON_PORT}/healthz"

        cargo build --quiet --locked --manifest-path "${BRIDGE_DIR}/Cargo.toml"
        BRIDGE_TOKEN="$BRIDGE_TOKEN_VALUE" \
        BRIDGE_SUBJECT_PREFIXES="nats.header.test." \
        BRIDGE_DURABLE_SUBJECT_PREFIXES="" \
        NATS_URL="nats://127.0.0.1:${NATS_PORT}" \
        PORT="$BRIDGE_PORT" \
          "${BRIDGE_DIR}/target/debug/nats-bridge" >"${WORK}/bridge.log" 2>&1 &
        BRIDGE_PID=$!
        wait_for 30 curl --fail --silent "${BRIDGE_URL}/readyz"

        matching="$(curl --silent --show-error \
          --request POST "${BRIDGE_URL}/publish/nats.header.test.match" \
          --header "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" \
          --header "x-bridge-token: ${BRIDGE_TOKEN_VALUE}" \
          --header 'x-message-id: header-contract-001' \
          --header 'idempotency-key: header-contract-001' \
          --header 'nats-msg-id: header-contract-001' \
          --header 'content-type: application/json' \
          --data '{"case":"matching"}')"
        python3 -c 'import json,sys; p=json.loads(sys.argv[1]); assert p["ok"] is True; assert p["durable"] is False; assert p["messageId"] == "header-contract-001"' "$matching"

        [[ "$(status \
          --request POST "${BRIDGE_URL}/publish/nats.header.test.auth" \
          --header "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" \
          --header 'x-bridge-token: wrong-token-0123456789' \
          --header 'content-type: application/json' \
          --data '{"case":"conflicting-auth"}')" == 401 ]]

        [[ "$(status \
          --request POST "${BRIDGE_URL}/publish/nats.header.test.message-id" \
          --header "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" \
          --header 'x-message-id: header-contract-002' \
          --header 'idempotency-key: header-contract-003' \
          --header 'content-type: application/json' \
          --data '{"case":"conflicting-message-id"}')" == 400 ]]

        metrics="$(curl --fail --silent "${BRIDGE_URL}/healthz")"
        python3 -c 'import json,sys; p=json.loads(sys.argv[1]); assert p["published_total"] == 1; assert p["core_published_total"] == 1; assert p["rejected_total"] == 2' "$metrics"

        echo "PASS matching aliases are accepted and conflicts fail closed"
