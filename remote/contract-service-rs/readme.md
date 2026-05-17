# `dd-contract-service`

Rust Solana contract gateway for the remote runtime.

The service does not store private keys and does not sign transactions. It validates declared
contract instruction envelopes, exposes a Solana-aware schema/example API, can simulate signed
transactions through a configured Solana JSON-RPC endpoint, and blocks `sendTransaction` unless
`SOLANA_SEND_ENABLED=true` is set explicitly.

## HTTP API

- `GET /healthz` - readiness/liveness probe.
- `GET /metrics` - Prometheus metrics.
- `GET /status` - checks `getHealth` and `getVersion` against `SOLANA_RPC_URL`.
- `GET /schema` - JSON Schema for `solana.contract.v1` envelopes.
- `GET /example` - minimal validation example.
- `POST /validate` - validates a contract instruction envelope and returns a deterministic digest.
- `POST /simulate` - calls Solana JSON-RPC `simulateTransaction` for a signed base64/base58 transaction.
- `POST /send` - calls Solana JSON-RPC `sendTransaction` only when `SOLANA_SEND_ENABLED=true`.

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
