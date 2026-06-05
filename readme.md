# `remote/deployments/fabrication-server-rs`

Rust fabrication planning service for additive, subtractive, turning, and
hybrid machine workflows.

It exposes:

- `GET /`
- `GET /healthz`
- `GET /readyz`
- `GET /metrics`
- `GET /docs/api`
- `GET /api/docs`
- `GET /api/docs.json`
- `POST /plan`
- `POST /fabrication/plan`
- `POST /instructions/analyze`

When `NATS_URL` is configured, the service also queue-subscribes to
`dd.remote.fabrication.requests` with queue group `dd-fabrication-server`,
publishes responses to `dd.remote.fabrication.results`, emits compact lifecycle
events to `dd.remote.events`, and can publish optimizer-shaped learning jobs to
`dd.remote.mdp.optimize` when `FABRICATION_MDP_AUTOPUBLISH=true`. Generated
machine code is intentionally advisory: responses are draft planning artifacts
and are not marked machine-ready.

## What It Does Today

`POST /fabrication/plan` accepts a fabrication intent, optional machine fleet,
optional part decomposition, optional existing instruction streams, and optional
learning hints. It returns:

- A normalized design summary with inferred additive, milling, turning, or
  special-process parts.
- A process plan across 3D printers, vertical/horizontal mills, routers, and
  lathes when those machine profiles are available.
- Draft machine programs such as Marlin-style printer G-code, ISO/Haas-style
  milling G-code, Fanuc-style turning G-code, or operator-only instructions for
  unsupported machine kinds.
- Validation findings and failure boundaries for heat-up, homing, spindle,
  work-offset, tool-change, manual-stop, deep-cut, arc, setup-limit,
  inspection, and automation constraints.
- Assembly advice that calls out when parts should be combined into one job or
  split so tight-tolerance features can be machined and inspected separately.
- A learning contract with MDP states, POMDP observations, policy actions,
  reward terms, neural feature names, and training-example sketches.

Real production use still requires CAD/CAM generation, controller-specific
post-processing, simulation, workholding review, material verification, and
operator sign-off.

## `POST /fabrication/plan`

`POST /plan` is an equivalent alias used by the gateway when public
`/fabrication/` traffic is prefix-stripped before it reaches this service.

Requests use camelCase JSON:

```json
{
  "requestId": "demo-hybrid-001",
  "objective": "PETG ergonomic handle with a machined threaded brass insert",
  "material": { "name": "PETG", "family": "polymer" },
  "toleranceMm": 0.12,
  "quantity": 2,
  "constraints": {
    "maxSetups": 4,
    "allowHumanIntervention": true,
    "allowMultiPartAssembly": true,
    "requireDryRun": true
  },
  "machines": [
    {
      "id": "prusa-xl",
      "kind": "fdm-printer",
      "controller": "marlin",
      "materials": ["PLA", "PETG", "ABS"],
      "workEnvelopeMm": [360, 360, 360],
      "axes": 3,
      "operations": ["additive-print"]
    },
    {
      "id": "tm1p",
      "kind": "vertical-mill",
      "controller": "haas-gcode",
      "materials": ["aluminum", "brass", "plastic"],
      "workEnvelopeMm": [760, 300, 400],
      "axes": 3,
      "operations": ["face", "pocket", "drill", "contour"]
    },
    {
      "id": "toolroom-lathe",
      "kind": "lathe",
      "controller": "fanuc-gcode",
      "materials": ["aluminum", "brass", "steel", "plastic"],
      "workEnvelopeMm": [300, 750],
      "axes": 2,
      "operations": ["face", "turn", "bore", "thread"]
    }
  ],
  "learning": {
    "modelFamily": "mdp-pomdp-neural-cam-policy",
    "policyHint": "prefer printed body plus turned insert after inspection succeeds",
    "observations": ["thread-gauge-pass", "insert-fit"],
    "rewardWeights": {
      "accuracy": 2,
      "interventionCost": -1
    }
  }
}
```

If `machines` is omitted, the service uses a conservative default fleet with an
FDM printer, vertical mill, horizontal mill, and lathe. If `parts` is omitted,
the planner infers a first decomposition from the objective, material, and
tolerance.

## `POST /instructions/analyze`

`POST /instructions/analyze` accepts existing G-code-like programs and returns
controller-agnostic safety findings plus improvement opportunities.

```json
{
  "requestId": "legacy-check-001",
  "programs": [
    {
      "id": "legacy-mill-op",
      "machineKind": "vertical-mill",
      "language": "gcode",
      "instructions": [
        "G21 G90 G54",
        "G1 Z-1.0 F100",
        "M0 flip fixture",
        "M30"
      ]
    }
  ]
}
```

The analyzer is intentionally conservative. It checks common `G`, `M`, and `T`
words, missing units or positioning modes, printer extrusion before heat-up or
homing, subtractive feed moves before spindle start, manual stops, fixture
changes, deep negative Z moves, arc moves without I/J/R geometry, and missing
program ends.

## Local Build

```bash
cd remote/deployments/fabrication-server-rs
cargo test
cargo run --release
```

The default local port is `8113`; set `PORT` to override it.
