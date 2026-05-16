# `dd-des-simulator`

Rust service for asynchronous discrete event simulations.

## HTTP API

- `GET /healthz` - readiness/liveness probe.
- `GET /metrics` - Prometheus metrics.
- `GET /model/schema` - declared JSON Schema for `des.v1` simulation requests.
- `GET /model/example` - runnable example request.
- `POST /validate` - validates a simulation request without scheduling work.
- `POST /simulate` - validates and starts an asynchronous simulation job, returning `202`.
- `GET /simulations/<jobId>` - returns queued/running/succeeded/failed job state.

## NATS API

The deployment subscribes to `dd.remote.des.simulate` with queue group `dd-des-simulator`.
Messages must use the same `des.v1` JSON request format as `POST /simulate`. Completed jobs are
published to `dd.remote.des.results`, and compact lifecycle events go to `dd.remote.events`.

## Model Format

Top-level request:

```json
{
  "requestId": "clinic-demo",
  "model": {
    "schemaVersion": "des.v1",
    "name": "clinic-intake",
    "timeUnit": "minutes",
    "eventTypes": [{ "name": "arrival" }, { "name": "done" }],
    "resources": [{ "name": "server", "capacity": 1 }],
    "initialEvents": [{ "at": 0, "eventType": "arrival", "entityId": "patient-1" }],
    "transitions": [
      {
        "name": "service",
        "from": "arrival",
        "to": "done",
        "delay": { "distribution": "fixed", "value": 0 },
        "resource": {
          "name": "server",
          "units": 1,
          "duration": { "distribution": "fixed", "value": 5 }
        }
      }
    ],
    "metrics": [{ "name": "completed", "eventType": "done", "kind": "count" }]
  },
  "options": { "until": 60, "maxEvents": 10000, "trace": true }
}
```

The Rust boundary validates the declared schema version, bounded request size, unique labels,
known event/resource references, finite non-negative times, probability ranges, resource capacity,
transition limits, and supported metric kinds before any simulation job is accepted.
