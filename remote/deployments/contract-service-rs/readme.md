# `dd-contract-service`

Rust Solana contract gateway for the remote runtime.

The service does not store private keys and does not sign transactions. It validates declared
contract instruction envelopes, exposes a Solana-aware schema/example API, offers a read-only
Solana RPC surface, simulates signed transactions, broadcasts settlement and dispute-resolution
transactions and confirms them to finality, and blocks every broadcast path behind explicit
enable flags plus separate auth secrets.

## HTTP API

### Contract validation / raw transactions

- `GET /healthz` - process liveness probe.
- `GET /readyz` - verifies Solana RPC, enabled PostgreSQL/Fiducia coordination, and formal-methods dependencies.
- `GET /capabilities` - explicit product boundary, integrations, supported escrow actions, and active gates.
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
- `POST /chain/signatures` - bounded `getSignaturesForAddress` history at durable commitment.
- `POST /chain/priority-fees` - bounded `getRecentPrioritizationFees` samples plus median/p90 summary.

### Programs, smart contracts, and formal verification

- `POST /program/inspect` - validates a program public key and proves the deployed account exists and
  is executable, returning owner, balance, data size, and context slot.
- `POST /program/verify` - submits either bounded inline Rust source to the synchronous formal-methods
  `/validate` endpoint or a repository job to `/analyses`. Repository URLs must be credential-free
  `https://github.com/fiducia-cloud/<repo>` URLs; refs and paths reject traversal and option injection.
- `POST /escrow/inspect` - checks that an escrow account exists, is owned by the expected program, is
  non-executable, meets an optional minimum balance, and is rent-exempt. `dd-escrow-rs` remains the
  domain validator for parties, terms, assets, release modes, and settlement policy.

These routes are stateless across the two replicas. The older optional watch/index/multisig registries
remain disabled in the checked-in deployment because they are bounded in-memory coordination aids,
not durable sources of truth.

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
- Every Solana `sendTransaction` call, regardless of whether it originated at `/send`, `/settle`,
  `/resolve`, NATS, executor, relayer, staking, or bridge code, passes through one broadcast fence. The
  service hashes the decoded signed transaction bytes, takes `pg_try_advisory_xact_lock` in a held
  Postgres transaction, and claims a durable Fiducia idempotency lease with a fencing token. Completed
  calls retain and replay the prior RPC result for seven days; failed calls abandon the lease. With
  `CONTRACT_COORDINATION_REQUIRED=true`, a coordinator failure blocks broadcasts instead of degrading
  to replica-local behavior.
- Coordination refuses to start without a `requests:write`-scoped `FIDUCIA_API_KEY`. Readiness uses a
  non-mutating `OPTIONS /v1/idempotency/claim` authorization probe, so a health-only connection cannot
  be mistaken for write access. The checked-in deployment keeps coordination and every broadcast flag
  disabled until fiducia-auth mints that scoped key into `dd-agent-secrets/FIDUCIA_API_KEY`; enabling any
  broadcast flag while coordination is disabled or optional is a startup error.
- Settlement/resolution request IDs are also claimed in-process for immediate caller replay feedback;
  the transaction-digest fence above is the cross-replica source of truth.
- Confirmation polling is bounded by a max timeout, min poll interval, and max poll count; `/confirm`
  accepts at most a small batch of signatures.
- A service-wide cap bounds concurrent confirmation pollers across `/confirm`, `/settle`, `/resolve`,
  and the escrow verifier, so no set of requests can amplify sustained outbound Solana RPC load. At the
  cap, confirmations are shed gracefully (reported as `deferred`, no RPC) rather than queued.
- `skipPreflight` is rejected unless `SOLANA_ALLOW_SKIP_PREFLIGHT=true`.
- `simulateTransaction` rejects `sigVerify=true` with `replaceRecentBlockhash=true`.
- Solana, Fiducia, and formal-methods HTTP clients refuse redirects and cap response bodies. A shared
  semaphore bounds concurrent Solana RPC requests (`SOLANA_RPC_MAX_IN_FLIGHT`, default 64).
- The deployment mounts the source checkout read-only and sends Cargo build/cache output to
  disposable `emptyDir` volumes.
- The Kubernetes pod runs as a non-root UID with a read-only root filesystem, no service-account
  token, dropped Linux capabilities, and a NetworkPolicy that allows only gateway/runtime-config/
  observability ingress plus explicit DNS, NATS, runtime-config, formal-methods, Fiducia, private RDS
  Postgres, and public HTTPS Solana RPC egress.

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

## Blockchain Feature Suite

In addition to the Solana contract gateway above, the service hosts ten **keyless, off-by-default**
blockchain modules (`src/blockchain/`). They extend the gateway with Solana **and** EVM read/relay
support while preserving the hardening contract: **no private keys are stored and nothing here
signs.** Anything that would sign goes through a pluggable `SignerBackend` whose only variant today is
`External` ("client must sign"); execute/relay paths accept an already client-signed raw transaction.
A future `Kms` variant is the documented custody extension point.

Every feature is gated by a `*_ENABLED` flag defaulting to `false`. Read/coordination paths need only
that flag; **execute/broadcast paths additionally require `CONTRACT_BLOCKCHAIN_AUTH_SECRET`** (sent as
the `x-contract-blockchain-auth` header) and, against a mainnet target,
`CONTRACT_BLOCKCHAIN_MAINNET_ENABLED=true` — a second gate mirroring the settlement mainnet gate. The
service refuses to start if an execute/broadcast flag is set without its secret (or against mainnet
without the mainnet flag). Registries/indexes are bounded in-memory and ephemeral (no Postgres DDL
from Rust, per the repo contract).

EVM support is optional: set `EVM_RPC_URL` (https, SSRF-guarded like `SOLANA_RPC_URL`), `EVM_CHAIN_ID`,
and `EVM_NETWORK`. EVM RPC is reached over the existing public-443 egress; ABI encoding is the
caller's responsibility (callers pass pre-encoded `data`).

### Modules and routes

1. **Blockchain core** — `GET /chains`, `GET /chain/:id/status` (`solana`/`evm`).
2. **Wallet management** (watch-only) — `POST /wallet/register`, `GET /wallet/list`,
   `POST /wallet/:id/balance`. `BLOCKCHAIN_WALLET_ENABLED`.
3. **Smart-contract executor** — `POST /executor/simulate` (Solana `simulateTransaction` / EVM
   `eth_call` + `eth_estimateGas`), `POST /executor/execute` (gated broadcast of a signed raw tx).
   `BLOCKCHAIN_EXECUTOR_ENABLED`, `BLOCKCHAIN_EXECUTOR_EXECUTE_ENABLED`.
4. **Transaction relayer** — `POST /relayer/submit` (stages, or broadcasts a signed raw tx when
   gated), `GET /relayer/status/:id`. `BLOCKCHAIN_RELAYER_ENABLED`,
   `BLOCKCHAIN_RELAYER_BROADCAST_ENABLED`.
5. **Multi-signature coordinator** (keyless) — `POST /multisig/proposal`,
   `POST /multisig/proposal/:id/approve`, `GET /multisig/proposal/:id`. `BLOCKCHAIN_MULTISIG_ENABLED`.
6. **Blockchain indexing** — `POST /index/watch` (optional one-shot poll), `GET /index/query`;
   publishes references to `BLOCKCHAIN_INDEX_EVENTS_SUBJECT`. `BLOCKCHAIN_INDEXER_ENABLED`.
7. **MEV/arbitrage monitoring** — **monitoring-only, no execution path.** `POST /mev/opportunities`
   computes the venue spread and flags an opportunity; alerts publish to `BLOCKCHAIN_MEV_ALERTS_SUBJECT`.
   `BLOCKCHAIN_MEV_ENABLED`.
8. **NFT/media storage** — `POST /nft/metadata/validate` (Metaplex / ERC-721/1155),
   `POST /nft/media` (content-addressed sha256), `GET /nft/media/:digest`. `BLOCKCHAIN_NFT_ENABLED`.
9. **Staking management** — `POST /staking/validate`, `GET /staking/positions/:chain/:address`,
   `POST /staking/intent` (gated broadcast). `BLOCKCHAIN_STAKING_ENABLED`,
   `BLOCKCHAIN_STAKING_EXECUTE_ENABLED`.
10. **Cross-chain bridge coordinator** (non-custodial) — `POST /bridge/transfer`,
    `POST /bridge/transfer/:id/attest` (verifies the source lock read-only),
    `GET /bridge/transfer/:id`; attestations publish to `BLOCKCHAIN_BRIDGE_ATTESTATIONS_SUBJECT`.
    `BLOCKCHAIN_BRIDGE_ENABLED`, `BLOCKCHAIN_BRIDGE_BROADCAST_ENABLED`.

### Telemetry

`/metrics` gains `dd_contract_service_blockchain_*` counters (requests, disabled rejections, auth
failures, blocked broadcasts, EVM RPC requests/errors, and per-feature activity). The three
publish-only subjects above are defined in `remote/libs/nats/subject-defs/schema/contracts.schema.json`
and consumed through the generated `dd_nats_subject_defs` constants (never hand-written strings).

> **ORM policy:** prefer **SeaORM** over sqlx for new database code (MASH stack: maud, axum, SeaORM, supabase, htmx).
