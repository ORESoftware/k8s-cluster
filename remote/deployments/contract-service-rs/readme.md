# `dd-contract-service`

Rust Solana contract gateway for the remote runtime.

The service does not store private keys and does not sign transactions. It validates declared
contract instruction envelopes, exposes a Solana-aware schema/example API, can simulate signed
transactions through a configured Solana JSON-RPC endpoint, and blocks `sendTransaction` unless
`SOLANA_SEND_ENABLED=true` is set explicitly with a separate `CONTRACT_SEND_AUTH_SECRET`.

## HTTP API

- `GET /healthz` - readiness/liveness probe.
- `GET /metrics` - Prometheus metrics.
- `GET /status` - checks `getHealth` and `getVersion` against `SOLANA_RPC_URL`.
- `GET /schema` - JSON Schema for `solana.contract.v1` envelopes.
- `GET /example` - minimal validation example.
- `POST /validate` - validates a contract instruction envelope and returns a deterministic digest.
- `POST /simulate` - calls Solana JSON-RPC `simulateTransaction` for a signed base64/base58 transaction.
- `POST /send` - calls Solana JSON-RPC `sendTransaction` only when `SOLANA_SEND_ENABLED=true` and
  the caller sends a matching `x-contract-send-auth` header.

## Hardening Contract

- Request `cluster` must match the configured `SOLANA_CLUSTER`; requests cannot relabel devnet RPC
  traffic as mainnet, or the reverse.
- `SOLANA_RPC_URL` must be HTTPS and must not point at localhost, private IPs, `.local`, or
  `.cluster.local` hosts unless `SOLANA_ALLOW_PRIVATE_RPC=true`.
- `sendTransaction` requires both `SOLANA_SEND_ENABLED=true` and `CONTRACT_SEND_AUTH_SECRET`.
- `skipPreflight` is rejected unless `SOLANA_ALLOW_SKIP_PREFLIGHT=true`.
- `simulateTransaction` rejects `sigVerify=true` with `replaceRecentBlockhash=true`.
- The deployment mounts the source checkout read-only and sends Cargo build/cache output to
  disposable `emptyDir` volumes.
- The Kubernetes pod runs as a non-root UID with a read-only root filesystem, no service-account
  token, dropped Linux capabilities, and a NetworkPolicy that allows only gateway/runtime-config/
  observability ingress plus DNS, NATS, runtime-config, and public HTTPS Solana RPC egress.

## Telemetry

- `/metrics` exposes Prometheus counters for HTTP traffic, contract validations, safety-policy
  rejections, Solana RPC requests/errors by fixed RPC method, NATS receive/publish outcomes,
  send-auth failures, and aggregate service errors.
- `dd-otel-collector` and `dd-prometheus` both scrape
  `dd-contract-service.default.svc.cluster.local:8101/metrics`; the generic Grafana Deployment
  Drilldown and Kubernetes Workload Fleet dashboards cover this deployment through the checked-in
  observability allowlist.
- Important runtime events write the shared `dd.log.v1` JSONL envelope to stdout/stderr, so
  Promtail/Loki can label the stream by `log_schema`, `severity`, and `log_service` while keeping
  request ids and error details as log fields.
- Invalid/oversized NATS validation messages and result-publish failures also publish compact
  critical events to `NATS_CRITICAL_EVENT_SUBJECT` (default `dd.remote.events.critical`) when NATS
  is available.

## NATS API

The deployment queue-subscribes to `dd.remote.contracts.solana.validate` with queue group
`dd-contract-service`. Messages use the same `solana.contract.v1` shape as `POST /validate`.
Validation results are published to `dd.remote.contracts.solana.results`, and compact lifecycle
events go to `dd.remote.events`.

## Contract Envelope

```json
{
  "schemaVersion": "solana.contract.v1",
  "requestId": "contract-demo",
  "cluster": "devnet",
  "programId": "11111111111111111111111111111111",
  "payer": "11111111111111111111111111111111",
  "recentBlockhash": "11111111111111111111111111111111",
  "commitment": "confirmed",
  "memo": "example contract instruction envelope",
  "instructions": [
    {
      "name": "system-transfer-shape",
      "accounts": [
        {
          "label": "from",
          "pubkey": "11111111111111111111111111111111",
          "isSigner": true,
          "isWritable": true
        },
        {
          "label": "to",
          "pubkey": "11111111111111111111111111111111",
          "isSigner": false,
          "isWritable": true
        }
      ],
      "dataBase64": "AQID",
      "computeUnits": 200000
    }
  ]
}
```

Validation checks schema version, base58 Solana public keys, supported clusters and commitments,
bounded instruction/account counts, instruction data encoding and size, memo size, request ID size,
and compute-unit limits before a request is accepted.
