#!/usr/bin/env bash
#
# Fetch the dd-remote-gateway TLS certificate and save it locally as a CA
# bundle that local MCP clients can pin via NODE_EXTRA_CA_CERTS. This keeps
# full TLS verification on (no NODE_TLS_REJECT_UNAUTHORIZED=0) while trusting
# the gateway's self-signed / IP cert.
#
# Output (gitignored by *.pem):  mcp/dd-gateway-ca.pem
#
# Usage:
#   ./mcp/fetch-gateway-ca.sh
#   DD_GATEWAY_HOST=my.gateway.example ./mcp/fetch-gateway-ca.sh
#
# If the gateway later serves a publicly-trusted cert (e.g. a Let's Encrypt
# IP/hostname cert), you can drop the NODE_EXTRA_CA_CERTS step entirely.
set -euo pipefail

HOST="${DD_GATEWAY_HOST:-98.90.186.114}"
PORT="${DD_GATEWAY_PORT:-443}"
OUT="$(cd "$(dirname "$0")" && pwd)/dd-gateway-ca.pem"

if ! command -v openssl >/dev/null 2>&1; then
  echo "error: openssl not found on PATH" >&2
  exit 1
fi

echo "Fetching TLS cert from ${HOST}:${PORT} ..." >&2
echo | openssl s_client -connect "${HOST}:${PORT}" -servername "${HOST}" 2>/dev/null \
  | openssl x509 -outform PEM > "${OUT}"

if [ ! -s "${OUT}" ]; then
  echo "error: no certificate written to ${OUT}" >&2
  exit 1
fi

echo "Wrote ${OUT}" >&2
openssl x509 -in "${OUT}" -noout -subject -issuer -dates >&2
echo >&2
echo "Point your MCP clients at this file:" >&2
echo "  export NODE_EXTRA_CA_CERTS=${OUT}" >&2
