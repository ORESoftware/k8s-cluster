# Go deployment API-contract inventory

This document is the first bounded implementation slice of DEN-843. It records the executable protocol boundary for every standalone Go module beneath `remote/deployments` and prevents operational listeners from being mistaken for customer-facing REST APIs.

The machine-readable source of truth is `remote/config/go-api-contract-inventory.json`. A permanent Node test recursively discovers every `go.mod` under `remote/deployments` and requires an exact one-to-one inventory match. Adding or removing a Go module without updating its classification fails CI.

## Classification rule

A process belongs in the OpenAPI and generated-SDK pipeline only when it dispatches a product HTTP request/response API. The existence of `net/http`, a TCP listener, a Prometheus endpoint, a Kubernetes health probe, a WebSocket upgrade, or an embedded runtime handler does not by itself make a process a REST API.

Every entry records:

- module root and executable entrypoint;
- process classification;
- primary protocol;
- operational HTTP routes, if any;
- whether a product HTTP API exists;
- the OpenAPI disposition;
- source-based evidence and the next required action.

Unknown or ambiguous modules must be classified before migration. The inventory does not permit a generic `unknown` value because that would allow the fleet program to claim coverage without deciding the real protocol boundary.

## Current standalone Go modules

### `go-wss-server-go` — protocol-only

The workload exists as the Go peer in a cross-language WebSocket benchmark. Its public protocol is a WebSocket frame contract on the benchmark listener. A separate admin listener exposes only `/metrics`, `/healthz`, and `/readyz`.

OpenAPI is not applicable to the WebSocket frame protocol. The correct follow-up is a versioned WebSocket protocol contract and conformance tests shared with the Rust, Dart, Gleam, and Akka peers—not fabricated REST paths or generated REST SDKs.

### `thread-operator-go` — controller/worker

The executable is a controller-runtime manager that watches and reconciles the `Thread` custom resource. Its network listeners are controller-runtime Prometheus and health/readiness servers. The product contract is the CRD, RBAC, reconciliation behavior, status transitions, and ownership rules.

OpenAPI is not applicable to the controller probes. The existing CRD and reconciliation tests remain authoritative.

### `thread-fleet-exporter-go` — metrics exporter

The exporter reads Kubernetes resources and publishes Prometheus metric families plus `/healthz`. It does not decode or dispatch product request DTOs.

OpenAPI and product SDK generation are not applicable. Its contract belongs to Prometheus exposition, metric-family stability, bounded labels, Kubernetes read permissions, and health behavior.

## Embedded Go source

`remote/deployments/container-pool-rs/runtime-images/common/golang-handler.go` is a runtime-image fixture copied and managed by `container-pool-rs`; it is not a standalone Go module or Kubernetes service. Any handler routes belong to the managed-function/container-pool executable contract and must not create a separate Go service identity.

## Central fleet policy

`remote/config/api-contracts.json` now records:

- Go product HTTP services must use framework-native typed registration or a repository-owned typed descriptor consumed by both dispatch and deterministic OpenAPI rendering;
- the Go classification inventory path;
- operational/protocol-only Go processes remain outside `legacySourceScannerAllowlist`.

When a future Go process is classified `product-http-api`, its same green change must add the executable contract, side-effect-free deterministic export, public/internal visibility filtering, standard runtime aliases, generated clients, runtime parity tests, and deployment/provenance evidence required by DEN-444/DEN-464. It must not enter the legacy regex scanner as a shortcut.

## Validation

`remote/tests/general/go-api-contract-inventory.test.ts` proves:

- every standalone Go deployment module is declared exactly once;
- IDs, roots, classifications, and operational routes are unique and stable;
- declared entrypoints and module roots exist;
- current classifications match the concrete source shape;
- protocol-only, controller, and exporter sources do not advertise standard REST docs routes;
- embedded sources remain separate from standalone modules;
- non-product Go processes are absent from the REST legacy scanner allowlist.

## Remaining DEN-843 work

This inventory closes the ambiguity around the currently indexed local Go modules. DEN-843 remains open for:

1. recurring discovery across gitlinked/upstream deployment repositories and any Go module not present in this checkout;
2. classification of other runtimes that remain outside the Rust, Node, Gleam, Dart, Python, Java, and F# lanes;
3. an executable typed contract migration for any future or newly discovered Go product HTTP API;
4. public/internal OpenAPI export, standard runtime aliases, SDK generation, live parity, and deployment provenance for those actual APIs;
5. retiring all false source-scanner assumptions only after native contract evidence exists.
