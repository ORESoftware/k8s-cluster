# GHA clone run-capacity boundary

The clone server retains run records in one process-local ordered map. `GHA_CLONE_MAX_RUNS` is a hard admission bound, not a best-effort pruning target.

## Atomic reservation

A direct run reserves one record. A workflow-run fallback reserves the complete mirrored-workflow batch. Reservation occurs under one write lock and either inserts every record or inserts none.

Before insertion, the server may evict only the oldest terminal records (`succeeded` or `failed`). Queued and running records are never evicted to admit new work. If active records plus the requested batch exceed the configured limit, admission returns HTTP `429` without changing the map. The bounded response reports only `error`, `maxRuns`, `activeRuns`, and `requestedRuns`; it does not expose retained run payloads, credentials, provider details, or workflow inputs.

Admission is based on the active-run invariant `activeRuns + requestedRuns <= maxRuns`. Terminal records can be removed only to make the retained-map size fit after that invariant succeeds. They can never compensate for insufficient active capacity.

Duplicate generated run IDs also fail without mutation. Equal-age terminal records are evicted deterministically by run ID.

## Webhook delivery identity

The webhook delivery UUID is claimed before reservation so concurrent duplicate deliveries cannot both dispatch. If atomic run reservation fails, that claim is removed before returning the capacity error. A later GitHub retry may therefore make progress after capacity becomes available; no partial run batch survives the failed attempt.

Once reservation succeeds, the delivery claim remains retained for the normal bounded TTL and the complete batch is dispatched.

## Process and durability boundary

This invariant is authoritative only inside one clone-server process. Keep the service at one replica while reservation and delivery claims remain in memory.

Before horizontal scaling or restart-transparent failover, move run-request identity, delivery claims, capacity admission, provider assignment, and fencing into Fiducia-backed durable state or another single shared consistency boundary. Do not add an independent local queue journal beside that authority.

A client retry after process loss must continue to use stable request/delivery identity. Fencing tokens are required before a recovered or replacement worker can claim execution authority without risking duplicate dispatch.

## Validation

The pure reservation module proves atomic success, mutation-free capacity failure, no active-run eviction, deterministic terminal eviction, duplicate-ID rejection, oversized-batch rejection, and zero-capacity failure. Real-process tests must additionally prove HTTP capacity responses, webhook claim rollback, retry progress, and no partial build submissions.
