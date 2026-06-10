# `dd-contract-service`

Rust Solana contract gateway for the remote runtime.

The service does not store private keys and does not sign transactions. It validates declared
contract instruction envelopes, exposes a Solana-aware schema/example API, offers a read-only
Solana RPC surface, simulates signed transactions, broadcasts settlement and dispute-resolution
transactions and confirms them to finality, and blocks every broadcast path behind explicit
enable flags plus separate auth secrets.

## HTTP API

### Contract validation / raw transactions

- `GET /healthz` - readiness/liveness probe.
- `GET /metrics` - Prometheus metrics.
- `GET /status` - checks `getHealth` and `getVersion` against `SOLANA_RPC_URL`.
- `GET /schema` - JSON Schema for `solana.contract.v1` envelopes.
- `GET /example` - minimal validation example.
- `POST /validate` - validates a contract instruction envelope and returns a deterministic digest.
- `POST /simulate` - calls Solana JSON-RPC `simulateTransaction` for a signed base64/base58 transaction.
- `POST /send` - calls Solana JSON-RPC `sendTransaction` only when `SOLANA_SEND_ENABLED=true` and
  the caller sends a matching `x-contract-send-auth` header.

### Read-only Solana RPC surface (gateway ingress only, no auth)

- `GET /blockhash` - `getLatestBlockhash`.
- `POST /account` - `getAccountInfo` for a validated base58 pubkey.
- `POST /balance` - `getBalance` (`kind:"sol"`, default) or `getTokenAccountBalance` (`kind:"token"`).
- `POST /fee` - `getFeeForMessage` for a base64 compiled message.
- `GET /rent-exemption?bytes=N` - `getMinimumBalanceForRentExemption` (bounded `N`).
- `POST /transaction` - `getTransaction` for a validated signature.
- `POST /confirm` - polls `getSignatureStatuses` until each signature reaches the target commitment
  (`confirmed`/`finalized`), fails on-chain, or the bounded timeout elapses.

### Settlement and dispute resolution

- `GET /schema/settlement`, `GET /schema/resolution`, `GET /example/settlement` - JSON Schemas/example.
- `POST /simulate-settlement` - validate + `simulateTransaction` for a `solana.settlement.v1` envelope
  (dry run, no broadcast, no auth).
- `POST /settle` - validate -> `sendTransaction` -> confirm-to-finality -> publish outcome. Requires
  `SOLANA_SETTLEMENT_ENABLED=true` and a matching `x-contract-settlement-auth` header.
- `POST /resolve` - dispute resolution: validates the `decision`->`action` mapping and arbiter signer,
  then the same broadcast+confirm+publish path. Requires `SOLANA_RESOLUTION_ENABLED=true` and the same
  settlement auth header.

Settlement actions reuse the `dd-escrow-rs` vocabulary (`fund`, `release`, `refund`, `partial-release`,
`split-release`, `dispute-award`, `expire`, `cancel`). Resolution decisions (`release-to-payee`,
`refund-to-payer`, `split`, `award-to-claimant`, `uphold`, `overturn`) each constrain which settlement
action may enact them.

## Hardening Contract

- Request `cluster` must match the configured `SOLANA_CLUSTER`; requests cannot relabel devnet RPC
  traffic as mainnet, or the reverse.
- `SOLANA_RPC_URL` must be HTTPS and must not point at localhost, private IPs, `.local`, or
  `.cluster.local` hosts unless `SOLANA_ALLOW_PRIVATE_RPC=true`.
- `sendTransaction` requires both `SOLANA_SEND_ENABLED=true` and `CONTRACT_SEND_AUTH_SECRET`.
- `/settle` and `/resolve` require `SOLANA_SETTLEMENT_ENABLED`/`SOLANA_RESOLUTION_ENABLED` plus
  `CONTRACT_SETTLEMENT_AUTH_SECRET` (a separate secret from the raw-send secret), checked in constant
  time.
- **Mainnet second gate.** When `SOLANA_CLUSTER=mainnet-beta`, the service refuses to start if any
  broadcast capability (`SOLANA_SEND_ENABLED`, `SOLANA_SETTLEMENT_ENABLED`, or
  `SOLANA_RESOLUTION_ENABLED`) is enabled without an explicit `SOLANA_MAINNET_SETTLEMENT_ENABLED=true`,
  so a single misconfigured flag cannot move real funds. Mirrors the dd-escrow-rs mainnet gate.
- **NATS-initiated broadcast is off by default.** NATS messages carry no auth header, so the `settle`/
  `resolve` subjects only validate, simulate, and confirm unless `CONTRACT_NATS_SETTLEMENT_ENABLED=true`
  (which additionally requires `SOLANA_SEND_ENABLED=true`).
- **Unauthenticated-bus acknowledgment.** The shared NATS bus currently has no per-subject
  authorization, so any pod that can reach it may publish to the `settle`/`resolve` subjects. To
  prevent NATS-triggered broadcast being enabled by flipping one boolean, the service refuses to
  start with `CONTRACT_NATS_SETTLEMENT_ENABLED=true` unless `CONTRACT_NATS_SETTLEMENT_ACK_UNAUTHENTICATED_BUS=true`
  is also set. Lock NATS down (subject authz + a messaging NetworkPolicy) before setting the ack.
- Settlement/resolution broadcasts are idempotent: an explicit `requestId` is claimed once within a
  bounded TTL window, so retries do not double-broadcast (HTTP returns `409` on replay).
- Confirmation polling is bounded by a max timeout, min poll interval, and max poll count; `/confirm`
  accepts at most a small batch of signatures.
- A service-wide cap bounds concurrent confirmation pollers across `/confirm`, `/settle`, `/resolve`,
  and the escrow verifier, so no set of requests can amplify sustained outbound Solana RPC load. At the
  cap, confirmations are shed gracefully (reported as `deferred`, no RPC) rather than queued.
- `skipPreflight` is rejected unless `SOLANA_ALLOW_SKIP_PREFLIGHT=true`.
- `simulateTransaction` rejects `sigVerify=true` with `replaceRecentBlockhash=true`.
- The deployment mounts the source checkout read-only and sends Cargo build/cache output to
  disposable `emptyDir` volumes.
- The Kubernetes pod runs as a non-root UID with a read-only root filesystem, no service-account
  token, dropped Linux capabilities, and a NetworkPolicy that allows only gateway/runtime-config/
  observability ingress plus DNS, NATS, runtime-config, and public HTTPS Solana RPC egress.

## Telemetry

- `/metrics` exposes Prometheus counters for HTTP traffic, contract validations, settlements,
  resolutions, idempotency suppressions, confirmation outcomes (`confirmed`/`finalized`/`failed`/
  `pending`/`deferred`), safety-policy rejections, Solana RPC requests/errors by fixed RPC method (now covering
  the full read surface), NATS receive/publish outcomes, send/settlement-auth failures, and aggregate
  service errors.
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

All subjects are defined in `remote/libs/nats/subject-defs/schema/contracts.schema.json` and consumed
through the generated `dd_nats_subject_defs` constants (never hand-written strings).

The deployment queue-subscribes (queue group `dd-contract-service`) to:

- `dd.remote.contracts.solana.validate` - `solana.contract.v1` envelopes, same shape as `POST /validate`.
- `dd.remote.contracts.solana.settle` - `solana.settlement.v1` envelopes.
- `dd.remote.contracts.solana.resolve` - `solana.resolution.v1` envelopes.

It optionally subscribes (when `CONTRACT_ESCROW_CONFIRM_ENABLED=true`, queue group
`dd-contract-service-escrow-confirm`) to `dd.remote.escrow.solana.results` and confirms any settlement
signature it carries to finality - the executor/verifier surface for `dd-escrow-rs`.

Validation results publish to `dd.remote.contracts.solana.results`; settlement, resolution, and escrow
confirmation outcomes publish to `dd.remote.contracts.solana.settlement.results`; compact lifecycle
events go to `dd.remote.events`; and oversized/invalid messages and publish failures emit compact
critical events to `NATS_CRITICAL_EVENT_SUBJECT` (default `dd.remote.events.critical`).

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
