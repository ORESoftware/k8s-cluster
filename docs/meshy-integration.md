# Meshy image-to-3D provider integration

Tracking: Linear `DEN-2465` · repository `daedalus-fab/fabrication-server.rs`

## Purpose

Meshy is an upstream geometry-generation provider for Daedalus. It can turn one image or one to four views of the same object into candidate 3D assets. Daedalus remains responsible for provenance, durable artifact storage, scale and topology review, mesh repair, manufacturing-route selection, simulation, inspection planning, and machine-release authorization.

A provider task in `SUCCEEDED` state is **not** a Daedalus machine release. Every task and candidate emitted by `dd-meshy-client` is deliberately represented as:

```json
{
  "releaseState": "draft",
  "machineReady": false
}
```

## Components

| Path | Responsibility |
| --- | --- |
| `crates/meshy-client/src/lib.rs` | Typed API client, validation, task models, redacted authentication, and Daedalus candidate envelopes. |
| `crates/meshy-client/src/cli.rs` | Automation-safe command surface. |
| `crates/meshy-client/src/bin/dd-meshy-adapter.rs` | Executable entry point. |
| `examples/meshy/*.json` | Single-view and four-view requests that explicitly ask for GLB, STL, and 3MF. |
| `scripts/ci/check_meshy_integration.py` | Credential-free source contract checked in CI. |

The provider crate is intentionally independently buildable. The top-level fabrication server still uses private path dependencies supplied by the `k8s-cluster` superproject, while this client can be compiled and tested in an ordinary GitHub Actions checkout.

## Authentication and configuration

`MESHY_API_KEY` is required. The client sends it as a sensitive `Authorization: Bearer ...` header and does not expose the value through `Debug` or structured errors.

`MESHY_API_BASE_URL` is optional and defaults to `https://api.meshy.ai`. Non-local plain HTTP endpoints are rejected. Localhost HTTP is accepted only to support deterministic mock-server tests.

Production deployment should inject `MESHY_API_KEY` from the cluster secret manager. Never commit the value, put it in a request body, forward it to a browser, or log the process environment.

## Commands

```text
dd-meshy-adapter capabilities
dd-meshy-adapter create-image <request.json|->
dd-meshy-adapter create-multi-image <request.json|->
dd-meshy-adapter get-image <task-id>
dd-meshy-adapter get-multi-image <task-id>
dd-meshy-adapter wait-image <task-id> [timeout-seconds]
dd-meshy-adapter wait-multi-image <task-id> [timeout-seconds]
dd-meshy-adapter list-image [page-number] [page-size]
dd-meshy-adapter list-multi-image [page-number] [page-size]
dd-meshy-adapter delete-image <task-id>
dd-meshy-adapter delete-multi-image <task-id>
```

Example:

```bash
export MESHY_API_KEY='...'

cargo run \
  --manifest-path crates/meshy-client/Cargo.toml \
  --bin dd-meshy-adapter \
  -- create-multi-image examples/meshy/multi-image-to-3d.json
```

The creation response is normalized to `dd.fabrication.external-generation-task.v1`. Retrieve or wait for the task to obtain `dd.fabrication.external-geometry-candidate.v1`, which includes provider URLs, expiry, credit usage, release blockers, and required evidence.

## Daedalus ingestion sequence

1. Store original JPG/PNG assets in Daedalus-controlled storage and calculate checksums.
2. Submit public short-lived URLs or data URIs to Meshy through this server-side client.
3. Persist the provider task id before retrying a submission; Meshy task creation is billable and should not be retried blindly.
4. Poll with bounded intervals or receive a Meshy webhook notification.
5. On a webhook, treat the payload as a notification and retrieve the task from Meshy again before accepting its state.
6. Immediately copy selected GLB/STL/3MF outputs from signed provider URLs into Daedalus-controlled storage and calculate hashes.
7. Submit the STL or 3MF through the existing design-input review and geometry-repair paths.
8. Require dimensional evidence, mesh/topology review, machine/material selection, toolpath simulation, inspection evidence, and release authorization.

## Request policy

The client rejects ambiguous or ignored combinations before a request reaches Meshy:

- exactly one of `input_task_id` or direct image input;
- one to four images for multi-image generation;
- JPEG/PNG URL or data-URI inputs;
- PBR only when textures are enabled;
- at most one texture prompt or texture image;
- 100–300,000 target polygons for standard remesh and 100–15,000 for `meshy-t2` Smart Topology;
- no remesh/topology settings that Smart Topology would silently ignore;
- explicit `3mf` request when Daedalus needs that package;
- `origin_at` and multi-view thumbnails only with automatic sizing;
- HTTPS for every non-local provider endpoint.

Automatic size estimation is advisory. Daedalus should prefer a known physical measurement, calibration object, scan, or inspection result before manufacturing.

## Failure and retry behavior

HTTP errors retain the status, provider code/message, and numeric `Retry-After` value when supplied. The API key is not retained in the public client structure or error value. Callers should use bounded exponential backoff for transport failures and `429`, honor account-wide queue limits, and avoid creating a replacement task until they know whether the first submission succeeded.

Task deletion is irreversible and removes provider-hosted models. Daedalus must complete durable ingestion before deleting a provider task.

## Next server integration

The next layer should expose authenticated Daedalus generation-job routes backed by persistent records and a worker queue. Those routes should call this crate rather than duplicate provider HTTP logic. The initial worker contract should include:

- Daedalus job id and idempotency key;
- source artifact ids and checksums;
- Meshy task kind and provider task id;
- normalized request JSON and model version;
- output artifact ids, hashes, media types, and expiry;
- credit estimate/actual usage;
- terminal provider state and error;
- immutable `machineReady: false` until existing release evidence clears.
