# `rust-network-mutex-rs` Wire Protocol

This file is the **single source of truth** for the JSON wire format between
clients (TypeScript / Rust / Gleam / Dart / Go) and the broker. Every client
under `clients/<lang>/` MUST mirror these enum variants exactly.

The Rust side is generated from `src/protocol.rs` by serde with:

```rust
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Request { … }
pub enum Response { … }
```

so the Rust types are the canonical schema. The cross-runtime clients all keep
their own type-safe enum (TS string-literal union, Go typed string + iota, Dart
sealed classes, Gleam custom types) so we never reach for magic strings the way
the upstream Node `live-mutex` library does.

## Transport

- **TCP / UDS**: newline-delimited JSON (`\n` terminates each frame). The
  broker accepts `EVENT_BYTE_LIMIT` (default 1 MiB) per frame.
- **HTTP**: separate `/api/v1/*` endpoints (see `readme.md`). HTTP requests
  are stateless; the broker manufactures an ephemeral client per call and
  detaches the lock before the request returns.

## Discriminator: `type`

Every request and response has a `type` field whose value is one of the camelCase
strings below. Implementations should switch on `type` (in a `match`/`switch`
that covers every variant — exhaustiveness checked by the language) instead of
sprinkling `if (msg.type === "lock")` strings around the code base.

### Request `type` values

| `type`           | Purpose                                                                    |
|------------------|----------------------------------------------------------------------------|
| `version`        | Client → broker version handshake                                          |
| `auth`           | Optional shared-secret auth (required when `LIVE_MUTEX_TOKEN` is set)      |
| `lock`           | Acquire an exclusive lock on `key` *or* a composite lock on `keys[]`       |
| `unlock`         | Release a lock; matches by `lockUuid` unless `force = true`                |
| `registerRead`   | RW: enqueue as a reader, granted when no writer is active                  |
| `registerWrite`  | RW: enqueue as a writer, granted when no readers/writer are active         |
| `endRead`        | RW: drop a reader hold                                                     |
| `endWrite`       | RW: drop the writer hold                                                   |
| `lockInfo`       | Inspect a key (lock count, holders, RW state)                              |
| `ls`             | List currently tracked keys                                                |
| `heartbeat`      | Keep-alive (no-op response on the broker)                                  |

### Response `type` values

| `type`                | Purpose                                                                    |
|-----------------------|----------------------------------------------------------------------------|
| `version`             | Broker version reply                                                       |
| `auth`                | Auth reply (`ok: true/false`, `error?`)                                    |
| `lock`                | Single-key lock grant or queued notification                               |
| `compositeLock`       | Multi-key lock grant (issued once *all* keys are held)                     |
| `unlock`              | Release ack                                                                |
| `registerReadResult`  | RW reader grant (`granted: true` once issued; queued otherwise)            |
| `registerWriteResult` | RW writer grant                                                            |
| `endReadResult`       | Ack from `endRead`                                                         |
| `endWriteResult`      | Ack from `endWrite`                                                        |
| `lockInfo`            | Reply to `lockInfo`                                                        |
| `lsResult`            | Reply to `ls`                                                              |
| `reelection`          | Broker hint that a queue head changed (for clients tracking position)      |
| `error`               | Generic error reply (correlated by `uuid`)                                 |
| `ok`                  | Generic success reply (correlated by `uuid`)                               |

## Field reference (subset — see `src/protocol.rs` for the full schema)

```jsonc
// Acquire: pick *exactly one* of `key` (single) or `keys` (composite).
{
  "type": "lock",
  "uuid": "<correlation-uuid>",
  "key": "k1",                // or omit and use:
  "keys": ["a","b","c"],      // up to 5; broker sorts for deadlock-free ordering
  "pid": 12345,               // optional, informational
  "ttl": 30000,               // ms; 0 = no TTL
  "max": null,                // per-key concurrency cap (semaphore). null/0 = leave cap as-is.
                              // 1 = mutex (default). N = up to N simultaneous holders.
                              // Broker silently clamps to LMX_MAX_CONCURRENCY_CAP (default 100).
                              // Composite locks (`keys`) always treat this as 1.
  "force": false,             // bypass holder check on contention
  "retryCount": 0,            // informational; broker doesn't retry
  "keepLocksAfterDeath": false
}

// Single-key grant:
{
  "type": "lock",
  "uuid": "<correlation-uuid>",
  "key": "k1",
  "acquired": true,
  "lockRequestCount": 0,
  "lockUuid": "L-…",          // store this; pass it back on unlock
  "fencingToken": 42,         // monotonically increasing per key
  "readersCount": 0
}

// Composite grant:
{
  "type": "compositeLock",
  "uuid": "<correlation-uuid>",
  "keys": ["a","b","c"],
  "acquired": true,
  "lockUuid": "L-…",
  "fencingTokens": { "a": 5, "b": 12, "c": 1 }
}

// Release:
{
  "type": "unlock",
  "uuid": "<correlation-uuid>",
  "key": "k1",
  "lockUuid": "L-…",
  "force": false
}
```

The full set of fields lives in `src/protocol.rs`. Cross-runtime clients should
keep their representations 1:1 with that file.

## Error correlation

Every reply carries the `uuid` of the originating request. Clients keep a map of
`uuid -> mpsc/channel` (or callback / future) so multiple in-flight requests
can multiplex over a single TCP/UDS connection. The Rust client (`src/client.rs`)
uses an `mpsc::UnboundedSender` so a single request can receive *more than one*
response — for example a `queued` notification followed by the actual grant.

Cross-runtime clients should follow the same pattern.

## Why an enum?

The upstream Node `live-mutex` broker uses bare `if (data.type === '…')`
strings sprinkled throughout `broker.js`, which means a typo silently routes
to "no handler" and a new variant has no compile-time enforcement. Our broker
matches a Rust enum (compiler-checked exhaustiveness), and every cross-runtime
client below uses the same pattern in its native idiom:

| Runtime    | Enum-style construct                                  |
|------------|-------------------------------------------------------|
| Rust       | `enum Request { … }` + serde tagged union              |
| TypeScript | discriminated union (`type Request = … \| …`)         |
| Go         | `type RequestType string` + typed const block + switch |
| Dart       | `sealed class Request` + pattern matching             |
| Gleam      | `pub type Request { Lock(…) RegisterRead(…) … }`      |

Adding a new variant in Rust is a compile error in every client until the
client adds the matching constructor — that is the property the upstream
library lacks.
