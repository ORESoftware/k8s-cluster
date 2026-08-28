#!/usr/bin/env bash
set -uo pipefail

EVIDENCE_PATH="${AI_BRIDGE_PUBLIC_EVIDENCE_PATH:-${RUNNER_TEMP:-/tmp}/ai-agent-bridge-public-ingress.json}"
CLASSIFIER="${AI_BRIDGE_PUBLIC_CLASSIFIER:-scripts/ci/classify-ai-agent-bridge-public-ingress.py}"
PROBE="${AI_BRIDGE_PUBLIC_PROBE:-scripts/ci/test-ai-agent-bridge-public-ingress.sh}"

if [[ ! -f "$CLASSIFIER" || ! -f "$PROBE" ]]; then
  echo "public ingress probe or classifier is missing" >&2
  exit 2
fi

set +e
AI_BRIDGE_PUBLIC_EVIDENCE_PATH="$EVIDENCE_PATH" bash "$PROBE"
probe_status=$?
set -e

if [[ ! -s "$EVIDENCE_PATH" ]]; then
  echo "public ingress probe did not produce evidence" >&2
  exit "$probe_status"
fi

set +e
python3 "$CLASSIFIER" --input "$EVIDENCE_PATH" --output "$EVIDENCE_PATH"
classifier_status=$?
set -e

# Diagnosis enriches evidence; it never converts a failed live probe into a
# passing workflow. Preserve either failure independently.
if [[ "$probe_status" != "0" ]]; then
  exit "$probe_status"
fi
if [[ "$classifier_status" != "0" ]]; then
  exit "$classifier_status"
fi
exit 0
