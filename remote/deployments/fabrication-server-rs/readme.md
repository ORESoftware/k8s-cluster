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
- `POST /fabrication/instructions/analyze`
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

The queue accepts direct plan payloads, direct instruction-analysis payloads
containing `programs`, rich fabrication outcome payloads containing `outcome`,
compact learning-outcome payloads containing `success`, and tagged envelopes
such as `{"type":"fabrication.instructions.analyze","request":{...}}`,
`{"type":"fabrication.learning.observe","request":{...}}`, or
`{"type":"fabrication.learning.outcome","request":{...}}`. Plan,
instruction-analysis, learning-observation, and compact learning-outcome results
are published to the fabrication result subject. Compact learning outcomes fan
out `fabrication.learning.outcome.result` with the retained policy snapshot.

## What It Does Today

`POST /fabrication/plan` accepts a fabrication intent, optional machine fleet,
optional part decomposition, optional existing instruction streams, and optional
learning hints. It returns:

Submitted `existingInstructions` are analyzed beside generated drafts. When the
request declares a material, those submitted programs are also checked against
resolved machine profile material lists before the plan is marked OK.

- A normalized design summary with inferred additive, milling, turning, or
  special-process parts.
- A process plan and structured `processGraph` across 3D printers,
  vertical/horizontal mills, routers, laser, waterjet, plasma/sheet cutters, and
  lathes when those machine profiles are available. The graph links operations,
  generated programs, sequencing dependencies, assembly interfaces, and release
  gates.
- Draft machine programs such as Marlin-style FDM printer G-code,
  SLA/MSLA resin print-wash-cure job sheets, SLS/MJF-style powder-bed
  print-cooldown-depowder job sheets, ISO/Haas-style vertical milling G-code,
  ISO-style horizontal side-slot/keyway milling G-code, GRBL-style router
  profile programs with tab gates, laser, waterjet, and plasma sheet-cutting
  job sheets with kerf tests and fire/fume gates, Fanuc-style turning G-code, or
  operator-only instructions for unsupported machine kinds.
- Validation and simulation findings plus failure boundaries for heat-up,
  homing, spindle, work-offset, additive material/color/tool-change,
  manual-stop, tool-length/probe compensation, canned drilling/tapping cycles,
  declared material/machine compatibility, additive support/orientation,
  additive thin-wall geometry, printer bed-adhesion, first-layer, fan-timing,
  resin-handling, powder-handling, deep-cut, arc, setup-limit,
  machine-envelope, sheet-cutting, inspection, and automation constraints.
- A `resolutionPlan` with ordered release-blocking remediation steps derived
  from failure boundaries, including split/combine, human review, automation,
  and regeneration phases.
- `improvements` and `improvedPrograms` review drafts for generated and
  submitted instruction streams, with conservative gates inserted before
  machine-ready release.
- Assembly advice with a structured `assemblyGraph` of part nodes, hybrid
  interface edges, join/fit strategies, inspection gates, and sequence steps for
  deciding when parts should be combined into one job or split so tight-tolerance
  features can be machined and inspected separately.
- A learning contract with MDP states, POMDP observations, policy actions,
  scored `strategyCandidates`, typed `interventionSignals`, reward terms,
  neural feature names, a deterministic neural-policy sketch, and
  training-example sketches. Failure boundary summaries, automation
  requirements, and resolution plans are converted into boundary-specific
  policy actions and observations so split, combine, human intervention,
  automation, and regeneration decisions can be learned from validation
  evidence.
- Outcome learning endpoints that accept fabrication results, shape reward
  terms, emit MDP/POMDP/neural evidence, and expose a bounded policy snapshot.
- Open-ended planning requests reuse strong learned method and assembly
  preferences from the bounded policy snapshot unless the caller supplies
  explicit process or join-strategy preferences.
- A bounded in-process job and artifact ledger for generated design summaries,
  parametric design payloads, process plans, machine programs, validation
  reports, boundary summaries, resolution plans, improved instructions,
  assembly plans, process graphs, assembly graphs, and optimizer-shaped MDP
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
FDM printer, SLA resin printer, SLS powder-bed printer, vertical mill,
horizontal mill, CNC router, laser cutter, waterjet cutter, plasma cutter, and
lathe. If `parts` is omitted, the planner infers a first decomposition from the
objective, material, and tolerance, including resin-print, powder-bed-print,
horizontal-milled side slots/keyways, laser, waterjet, plasma, and
kerf-controlled sheet-cut profiles, and routed sheet/profile parts for wood,
foam, acrylic, panel, sign, engraving, and tabbed-profile requests. Additive
plans flag overhang, bridge, cantilever, thin-wall, snap-fit, and resin
drain/cupping geometry as review boundaries before draft machine instructions
are treated as releasable.

## `POST /instructions/analyze`

`POST /fabrication/instructions/analyze` and its gateway-stripped alias
`POST /instructions/analyze` accept existing G-code-like programs plus
non-controller text instructions such as printer job sheets, setup sheets, and
operator checklists. It returns controller-agnostic safety findings, improvement
opportunities, and `improvedPrograms` review drafts that insert conservative
modal defaults or explicit setup, post-processing, split, assembly, and
human-intervention gates. Submitted machine profiles are bounded and validated,
including positive work-envelope values, unique IDs, and nonzero axis counts.

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
homing, missing bed-temperature waits, first-layer adhesion setup, early
part-cooling fan timing, additive material/color/tool-change stops such as
`M600` or multi-tool selection, mill/router plunges after tool selection
without explicit `G43`/probe/tool-length state, unsafe canned
drilling/peck/tapping cycles, subtractive feed moves before spindle start,
lathe constant-surface-speed without a spindle cap, threading cycles, part-off
or cutoff operations, manual stops, fixture changes, deep negative Z moves, arc
moves without I/J/R geometry, missing program ends, declared material
incompatibility with resolved machine profiles, and text-instruction boundaries
where the job needs setup, post-processing, resin IPA/wash/cure/waste controls,
powder cooldown/depowder/recovery controls, sheet-cutting kerf/fire/fume checks,
assembly, splitting, or operator intervention. Improved drafts are still marked
`machineReady=false`; they are normalization aids for review, motion-envelope
simulation, and controller-specific postprocessing.
`resolutionPlan` converts those boundaries into ordered remediation steps before
a human or downstream agent attempts machine-ready release.

Machine-code planning and analysis also run a bounded coordinate-envelope
simulation over `G0`/`G1`/arc motion. When a submitted or generated toolpath
exceeds the selected machine `workEnvelopeMm`, the service emits
`simulated-axis-envelope-exceeded` findings, `simulated-machine-envelope`
failure boundaries, and a retained `simulation-report` or
`analysis-simulation-report` artifact. Mill/router rapid lateral moves at or
below the stock surface emit `simulated-rapid-below-clearance` findings and a
`simulated-rapid-clearance` boundary so clamp, tab, fixture, and stock-collision
risks are reviewed before release.

Plan and analysis responses include a `boundarySummary` object that rolls raw
failure boundaries into operator-facing counts, typed `automationRequirements`,
and recommended actions: human-review, split-job-or-part,
combine-or-assemble-parts, add-verified-automation,
regenerate-or-repostprocess, and resolve-machine-failure-risk. Each response
also includes a `resolutionPlan` that orders those actions into release gates
before generated or improved instructions can be treated as machine-ready. The
same data is retained as `boundary-summary`, `analysis-boundary-summary`,
`resolution-plan`, or `analysis-resolution-plan` artifacts.

Plan responses also include `assembly.assemblyGraph`; the retained
`parametric-design` and `assembly-plan` artifacts carry the same graph so
external CAD/CAM or learning workers can connect generated parts, manufacturing
methods, join interfaces, dry-fit/metrology gates, and assembly sequence steps.
Plan responses and the retained `process-graph`, `parametric-design`, and
`mdp-request` artifacts include `processGraph` nodes, dependencies, and release
gates so downstream agents can reason over operation order, generated programs,
assembly-interface dependencies, and validation gates without reparsing prose.

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

When a policy snapshot has at least two positive samples for a method such as
`additive-print`, `milling`, `horizontal-milling`, `routing`, `sheet-cutting`,
or `turning`, subsequent `/fabrication/plan` requests without explicit
`preferredMethods` inherit those learned process preferences. Repeated
multi-method successes such as `additive-print+milling` are retained as method
combination preferences; open future requests can be decomposed into learned
hybrid parts before machine selection. Strong assembly preferences such as
`printed body plus turned insert` are reused as learned hybrid join strategies,
and recent neural training examples are carried into the returned learning plan.
The plan also includes scored `strategyCandidates` such as selected hybrid,
additive consolidation, machined datum-finish, and split-for-inspection options.
These candidates carry methods, machine kinds, estimated time, intervention
counts, boundary counts, scores, and rationale so the MDP/POMDP optimizer can
compare alternate make strategies instead of only seeing the selected route. A
`neuralPolicy` sketch with a normalized feature vector, hidden activations, and
bounded action scores lets an external neural model train from the same state or
replace the local scoring head. `interventionSignals` expose automation
requirements and ordered `resolutionPlan` steps as learnable actions,
observations, next states, and reward adjustments. The optimizer-shaped
`mdp-request` artifact includes `strategyCandidates`, `interventionSignals`,
`automationRequirements`, and `resolutionPlan` so external MDP/POMDP workers can
learn from the same boundary evidence.

## Job And Artifact Inspection

Every successful planning, instruction-analysis, learning-observation, or
learning-outcome request is recorded in a
bounded in-process ledger. This is not durable storage yet; it is the current
runtime inspection boundary while the database contract is still being designed.

- `GET /jobs` lists retained jobs with status, severity, summary, and artifact
  IDs.
- `GET /jobs/:job_id` returns the recorded plan or analysis response plus
  artifact summaries.
- `GET /jobs/:job_id/artifacts/:artifact_id` returns one full artifact payload,
  such as `design-summary`, `parametric-design`, `process-plan`,
  `process-graph`, `boundary-summary`, `simulation-report`, `learning-plan`,
  `mdp-request`, a `program-*` generated machine program, or an
  `improved-program-*` instruction rewrite, plus instruction-analysis artifacts such as
  `analysis-boundary-summary`, `analysis-simulation-report`, and learning
  artifacts such as `reward-signal`, `mdp-experience`, `pomdp-observations`, and
  `neural-example`. `parametric-design` and `assembly-plan` include
  `assemblyGraph` nodes, interfaces, and sequence gates; `parametric-design`,
  `process-graph`, and `mdp-request` include `processGraph` operation nodes,
  dependencies, and release gates.

## Local Build

```bash
cd remote/deployments/fabrication-server-rs
cargo test
cargo run --release
```

The default local port is `8113`; set `PORT` to override it.
