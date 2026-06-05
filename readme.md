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
- `GET /jobs`
- `GET /jobs/:job_id`
- `GET /jobs/:job_id/artifacts/:artifact_id`
- `GET /learning/policy`
- `GET /fabrication/learning/policy`
- `POST /plan`
- `POST /fabrication/plan`
- `POST /instructions/analyze`
- `POST /learning/observe`
- `POST /fabrication/learning/observe`
- `POST /learning/outcomes`
- `POST /fabrication/learning/outcomes`

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
  machine-envelope, inspection, and automation constraints.
- Assembly advice that calls out when parts should be combined into one job or
  split so tight-tolerance features can be machined and inspected separately.
- A learning contract with MDP states, POMDP observations, policy actions,
  reward terms, neural feature names, and training-example sketches.
- Outcome learning endpoints that accept fabrication results, shape reward
  terms, emit MDP/POMDP/neural evidence, and expose a bounded policy snapshot.
- A bounded in-process job and artifact ledger for generated design summaries,
  parametric design payloads, process plans, machine programs, validation
  reports, improved instructions, assembly plans, and optimizer-shaped MDP
  requests.

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

`POST /instructions/analyze` accepts existing G-code-like programs plus
non-controller text instructions such as printer job sheets, setup sheets, and
operator checklists. It returns controller-agnostic safety findings, improvement
opportunities, and `improvedPrograms` review drafts that insert conservative
modal defaults or explicit setup, post-processing, split, assembly, and
human-intervention gates.

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
changes, deep negative Z moves, arc moves without I/J/R geometry, missing
program ends, and text-instruction boundaries where the job needs setup,
post-processing, assembly, splitting, or operator intervention. Improved drafts
are still marked `machineReady=false`; they are normalization aids for review,
simulation, and controller-specific postprocessing.

## Outcome Learning

`POST /fabrication/learning/observe` accepts completed or failed fabrication
outcomes for a generated plan, program, part, machine, or external shop-floor
instruction stream. `POST /learning/observe` is the gateway-stripped alias.

The route validates bounded observations, optional dimensional/surface/time
measurements, machine failure flags, scrap flags, human-intervention cost, and
optional reward weights. It returns:

- A shaped reward with per-term contributions for completion, machine failure,
  scrap, human intervention, dimensional accuracy, surface quality, and machine
  time.
- An MDP experience update, POMDP observation list, and neural training example.
- A policy snapshot summarizing retained method and assembly preferences.

Learning outcomes are also recorded as job artifacts: `outcome-learning-event`,
`reward-signal`, `mdp-experience`, `pomdp-observations`, and `neural-example`.
`GET /fabrication/learning/policy` and `GET /learning/policy` return the
current bounded in-process policy memory. `POST /learning/outcomes` and
`POST /fabrication/learning/outcomes` accept a compact success/reward record
when callers already have their own training features.

## Job And Artifact Inspection

Every successful planning, instruction-analysis, or learning-observation
request is recorded in a
bounded in-process ledger. This is not durable storage yet; it is the current
runtime inspection boundary while the database contract is still being designed.

- `GET /jobs` lists retained jobs with status, severity, summary, and artifact
  IDs.
- `GET /jobs/:job_id` returns the recorded plan or analysis response plus
  artifact summaries.
- `GET /jobs/:job_id/artifacts/:artifact_id` returns one full artifact payload,
  such as `design-summary`, `parametric-design`, `process-plan`,
  `learning-plan`, `mdp-request`, a `program-*` generated machine program, or an
  `improved-program-*` instruction rewrite, plus learning artifacts such as
  `reward-signal`, `mdp-experience`, `pomdp-observations`, and
  `neural-example`.

## Local Build

```bash
cd remote/deployments/fabrication-server-rs
cargo test
cargo run --release
```

The default local port is `8113`; set `PORT` to override it.
