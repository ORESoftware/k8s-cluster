# `dd-escrow-rs`

Rust Solana escrow intent validator and settlement gateway.

The service does not store private keys and does not sign transactions. Callers submit escrow
intents plus client-signed settlement transactions. `dd-escrow-rs` validates escrow-specific policy,
can simulate settlement transactions through Solana JSON-RPC, and only sends transactions when
`SOLANA_SETTLEMENT_ENABLED=true` and `ESCROW_SETTLEMENT_AUTH_SECRET` are configured.

## Escrow Kinds

The `solana.escrow.v1` catalog supports ten escrow shapes:

- `marketplace-order`
- `milestone`
- `freelance-contract`
- `digital-delivery`
- `otc-trade`
- `rental-deposit`
- `bounty`
- `subscription-release`
- `group-buy`
- `dispute-resolution`

Each kind has allowed party roles, release modes, and settlement actions. Validation catches missing
required roles, malformed Solana public keys, invalid asset requirements, oversized memo/metadata
payloads, unsafe time windows, bad milestone splits, invalid payout splits, and settlement actions
that do not belong to the selected kind.

## HTTP API

- `GET /healthz` - liveness/readiness probe.
- `GET /metrics` - Prometheus metrics.
- `GET /status` - checks `getHealth` and `getVersion` against `SOLANA_RPC_URL`.
- `GET /types` - escrow kind catalog.
- `GET /schema` - JSON Schema sketch for `solana.escrow.v1`.
- `GET /example` - marketplace escrow example.
- `POST /validate` - validates an escrow intent and returns a deterministic digest.
- `POST /simulate-settlement` - calls Solana JSON-RPC `simulateTransaction` for a signed settlement
  transaction.
- `POST /settle` - calls Solana JSON-RPC `sendTransaction` only when explicitly enabled and the
  caller sends the matching `x-escrow-settlement-auth` header.

Generated docs are served at `/docs/api`, `/api/docs`, and `/api/docs.json`.

## Hardening Contract

- Request `cluster` must match configured `SOLANA_CLUSTER`.
- `SOLANA_RPC_URL` must use HTTPS and must not point at localhost, private IPs, `.local`, or
  `.cluster.local` hosts unless `SOLANA_ALLOW_PRIVATE_RPC=true`.
- `POST /settle` requires both `SOLANA_SETTLEMENT_ENABLED=true` and
  `ESCROW_SETTLEMENT_AUTH_SECRET`.
- `skipPreflight` is rejected unless `SOLANA_ALLOW_SKIP_PREFLIGHT=true`.
- Settlement transactions must already be signed by the caller and fit within the bounded payload
  size.
- The Kubernetes deployment runs as a non-root UID, disables service-account token mounting, uses a
  read-only root filesystem, and limits ingress/egress with NetworkPolicy.

## NATS API

When `NATS_URL` is configured, the deployment queue-subscribes to
`dd.remote.escrow.solana.validate` with queue group `dd-escrow-rs`. Messages use the same
`solana.escrow.v1` intent shape as `POST /validate`. Results are published to
`dd.remote.escrow.solana.results`, lifecycle events go to `dd.remote.events`, and alert-worthy
failures publish compact critical events to `dd.remote.events.critical`.

## Example Intent

```json
{
  "schemaVersion": "solana.escrow.v1",
  "requestId": "escrow-demo",
  "cluster": "devnet",
  "kind": "marketplace-order",
  "escrowId": "order.demo.001",
  "parties": [
    {
      "role": "buyer",
      "pubkey": "11111111111111111111111111111111",
      "label": "buyer",
      "requiredSigner": true
    },
    {
      "role": "seller",
      "pubkey": "11111111111111111111111111111111",
      "label": "seller",
      "payoutBps": 10000
    }
  ],
  "asset": {
    "assetType": "sol",
    "amountLamports": 1000000,
    "escrowVault": "11111111111111111111111111111111"
  },
  "terms": {
    "releaseMode": "buyer-approval",
    "settlementActions": ["fund", "release", "refund", "dispute-award"],
    "disputeWindowSeconds": 604800,
    "inspectionPeriodSeconds": 172800,
    "timeoutUnixSeconds": 1800000000,
    "requiredApprovals": ["buyer"],
    "deliveryRequired": true
  },
  "settlementPlan": {
    "programId": "11111111111111111111111111111111",
    "feeBps": 50,
    "memoRequired": true
  },
  "memo": "example marketplace escrow intent",
  "metadata": {
    "source": "dd-escrow-rs-example"
  }
}
```
