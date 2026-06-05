# `remote/deployments/fabrication-server-rs`

Rust fabrication planning service for additive, subtractive, and hybrid machine
workflows.

It exposes:

- `GET /healthz`
- `GET /metrics`
- `GET /schema`
- `GET /example`
- `POST /fabricate`
- `POST /instructions/validate`
- `POST /instructions/improve`
- `POST /learn/telemetry`
- `GET /docs/api`, `GET /api/docs`, and `GET /api/docs.json`

The service also queue-subscribes to `dd.remote.fabrication.requests` with queue
group `dd-fabrication-server`, publishes responses to
`dd.remote.fabrication.results`, and emits compact lifecycle events to
`dd.remote.events`.

## What it does today

This is a deterministic planning and validation kernel, not an unattended
hardware controller. It accepts a fabrication intent, machine capabilities,
optional existing instruction streams, and optional learning configuration. It
then returns:

- A normalized design package with a parametric placeholder artifact.
- A multi-operation fabrication plan across printers, mills, routers, lathes,
  cutters, inspection, and assembly.
- Conservative generated instruction artifacts such as Marlin/Fanuc/Haas-style
  G-code wrappers or setup sheets.
- Boundary findings for machine envelopes, tooling, support material, feed,
  spindle, thermal limits, missing homing, missing positioning modes, tool
  changes, pauses, split requirements, and assembly requirements.
- A learning plan that describes MDP/POMDP states, actions, observations,
  reward signals, hidden-state hints, and neural-training feature hints.

Generated machine code is intentionally advisory. Real production use still
requires a checked CAD/CAM artifact, controller-specific post-processing,
simulation, workholding review, and operator sign-off.

## `POST /fabricate`

Requests use camelCase JSON:

```json
{
  "requestId": "demo-bracket-001",
  "objective": {
    "name": "sensor bracket",
    "material": "PETG",
    "quantity": 2,
    "boundingBoxMm": { "x": 90, "y": 45, "z": 28 },
    "toleranceMm": 0.08,
    "overhangDegrees": 55,
    "minWallMm": 1.2,
    "strengthPriority": 0.8
  },
  "availableMachines": [
    {
      "id": "prusa-xl",
      "kind": "fdmPrinter",
      "capabilities": {
        "workEnvelopeMm": { "x": 360, "y": 360, "z": 360 },
        "materials": ["PLA", "PETG", "ABS"],
        "nozzleDiameterMm": 0.4,
        "minLayerHeightMm": 0.08,
        "minToleranceMm": 0.2,
        "maxExtruderTempC": 300,
        "maxBedTempC": 120
      }
    },
    {
      "id": "tm1p",
      "kind": "verticalMill",
      "capabilities": {
        "workEnvelopeMm": { "x": 760, "y": 300, "z": 400 },
        "materials": ["aluminum", "steel", "plastic"],
        "toolDiametersMm": [3.175, 6, 10],
        "maxSpindleRpm": 6000,
        "maxFeedMmMin": 1200,
        "minToleranceMm": 0.03
      }
    }
  ],
  "learning": {
    "mode": "hybrid",
    "horizon": 6,
    "rewardWeights": [
      { "signal": "toleranceHit", "weight": 2 },
      { "signal": "humanInterventionMinutes", "weight": -1 }
    ]
  }
}
```

Responses include `design`, `plan`, generated `instructions`, validation
results for supplied instruction streams, `boundaries`, and a `learning` block.

## Instruction validation

`POST /instructions/validate` accepts existing G-code-like payloads and flags
controller-agnostic safety boundaries. `POST /instructions/improve` returns the
same analysis plus an advisory wrapper that inserts explicit millimeter,
absolute-positioning, homing, and review comments when they are missing.

The validator is intentionally conservative. It parses common `G`, `M`, and
`T` words, estimates axis bounds, checks machine envelope and configured feed,
spindle, bed, and extruder limits, and reports human-intervention commands such
as `M0`, `M1`, `M6`, and `M600`.

## Learning path

`POST /learn/telemetry` turns fabrication telemetry into reward-style signals
for future policy optimization. The response is shaped for delegation to the
existing `dd-mdp-optimizer` service and for a future neural policy service:

- MDP/POMDP state and action labels are emitted by `/fabricate`.
- Telemetry events become bounded reward signals.
- Hidden-state hints track tool wear, material batch variation, fixture
  rigidity, and operator availability.
- Neural examples are textual feature sketches only; the service does not train
  or run a neural network in-process.

## Kubernetes layout

- `remote/argocd/dd-next-runtime/dd-fabrication-server.deployment.yaml`
  runs the Rust binary from the host-mounted checkout on port `8113`.
- `remote/argocd/dd-next-runtime/dd-fabrication-server.service.yaml`
  exposes the ClusterIP Service.
- `remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml`
  routes authenticated `/fabrication/` traffic to the service.
- `remote/argocd/dd-next-runtime/availability-pdbs.yaml` keeps at least one
  ready pod during voluntary disruption.

## Local build

```bash
cd remote/deployments/fabrication-server-rs
cargo test
cargo run --release
```

Then open `http://127.0.0.1:8113/example` for a request payload.
