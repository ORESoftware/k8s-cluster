# `dd-escrow-rs`

Rust Solana escrow intent validator and settlement gateway.

The service does not store private keys and does not sign transactions. Callers submit escrow
intents plus client-signed settlement transactions. `dd-escrow-rs` validates escrow-specific policy,
can simulate settlement transactions, and only sends transactions when
`SOLANA_SETTLEMENT_ENABLED=true` and `ESCROW_SETTLEMENT_AUTH_SECRET` are configured. Live
settlement requires an attached validated intent by default.

## Settlement Backend

On-chain `simulate`/`send` are routed through a pluggable backend selected by
`ESCROW_SETTLEMENT_BACKEND`:

- `contract-service` (default) - delegates to the in-cluster `dd-contract-service` Solana gateway at
  `CONTRACT_SERVICE_URL` (`/simulate` and `/send`). `/send` attaches the `x-contract-send-auth`
  header from `CONTRACT_SERVICE_SEND_AUTH_SECRET`. The escrow service keeps all of its local policy
  gates (settlement enabled, auth header, intent + resolution validation) and only delegates the raw
  RPC step, so contract-service stays the single Solana egress point in the cluster.
- `solana-rpc` - calls Solana JSON-RPC (`SOLANA_RPC_URL`) directly, the original behavior, kept as a
  fallback.

`GET /capabilities` and `GET /status` report the active backend and contract-service reachability.

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
- `GET /status` - reports the active settlement backend, probes `dd-contract-service` health when
  delegating, and checks `getHealth`/`getVersion` against `SOLANA_RPC_URL`.
- `GET /types` - escrow kind catalog.
- `GET /capabilities` - runtime settlement posture, limits, and supported escrow kinds.
- `GET /schema` - JSON Schema sketch for `solana.escrow.v1`.
- `GET /example` - marketplace escrow example.
- `POST /validate` - validates an escrow intent and returns a deterministic digest.
- `POST /audit` - returns validation, readiness, checks, warnings, and errors without touching
  Solana RPC.
- `POST /resolve` - validates a proposed resolution (`{ action, intent, resolution }`) against the
  escrow parties without touching Solana. See Resolutions below.
- `POST /simulate-settlement` - simulates a signed settlement transaction through the active
  settlement backend (`simulateTransaction`).
- `POST /settle` - sends a signed settlement transaction through the active settlement backend
  (`sendTransaction`) only when explicitly enabled and the caller sends the matching
  `x-escrow-settlement-auth` header.

Generated docs are served at `/docs/api`, `/api/docs`, and `/api/docs.json`.

## Resolutions

Settlement and `/resolve` requests may attach a `resolution` block describing the intended outcome,
which is cross-checked against the escrow parties so the chosen settlement action is consistent with
who is actually in the escrow:

- `outcome` (one of `release`, `refund`, `split`, `dispute-award`, `expire`, `cancel`) must map onto
  the settlement `action` (`split` accepts `split-release` or `partial-release`).
- `refund` requires a refundable party (buyer, payer, depositor, client, tenant, or contributor); an
  explicit `refundRole` must be both refundable and present.
- `dispute-award` requires a `winnerRole` that is a present, non-arbitrator party, and - under
  `arbiter-decision`/`multi-sig` release modes - an `arbitrator` party.
- `split` requires `allocations` whose `payoutBps` sum to exactly 10000, each referencing a present
  party role (and valid `pubkey` when provided).

A `resolution` on `/settle` or `/simulate-settlement` requires an attached `intent` so the outcome
can be checked against the escrow parties.

## Hardening Contract

- Request `cluster` must match configured `SOLANA_CLUSTER`.
- `SOLANA_RPC_URL` must use HTTPS and must not point at localhost, private IPs, `.local`, or
  `.cluster.local` hosts unless `SOLANA_ALLOW_PRIVATE_RPC=true`.
- `POST /settle` requires both `SOLANA_SETTLEMENT_ENABLED=true` and
  `ESCROW_SETTLEMENT_AUTH_SECRET`.
- `POST /settle` requires an attached `intent` by default
  (`ESCROW_SETTLEMENT_REQUIRE_INTENT=true`), so live sends are checked against the escrow parties,
  terms, asset, action, and settlement plan.
- Mainnet settlement has a second explicit gate:
  `SOLANA_MAINNET_SETTLEMENT_ENABLED=true`.
- `ESCROW_ALLOWED_PROGRAM_IDS` can hold a comma-separated Solana program allowlist. When set,
  `settlementPlan.programId` must be one of those public keys.
- `skipPreflight` is rejected unless `SOLANA_ALLOW_SKIP_PREFLIGHT=true`.
- Settlement transactions must already be signed by the caller and fit within the bounded payload
  size.
- `ESCROW_SETTLEMENT_BACKEND=contract-service` requires `CONTRACT_SERVICE_URL`. The URL must be an
  absolute `http`/`https` address without embedded credentials (in-cluster `*.svc.cluster.local`
  hosts are allowed). Live delegated sends additionally require both this service's send gates and
  the contract-service `SOLANA_SEND_ENABLED=true` + matching `x-contract-send-auth`.
- The Kubernetes deployment runs as a non-root UID, disables service-account token mounting, uses a
  read-only root filesystem, and limits ingress/egress with NetworkPolicy. Egress is restricted to
  DNS, NATS, runtime-config, `dd-contract-service:8101`, and public HTTPS (retained for the
  `solana-rpc` fallback backend and `/status` probes).

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
