# `remote/deployments/fabrication-server-rs`

Rust fabrication planning service for additive, subtractive, turning, and
hybrid machine workflows.

It exposes:

- `GET /`
- `GET /healthz`
- `GET /readyz`
- `GET /metrics`
- `GET /capabilities`
- `GET /fabrication/capabilities`
- `GET /schema`
- `GET /fabrication/schema`
- `GET /examples`
- `GET /fabrication/examples`
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
- A `designPackage` with per-part parametric primitives, coordinate frames,
  neutral export targets such as 3MF, STL, STEP, DXF, CAM setup JSON, and
  assembly graph exports for downstream CAD/CAM, slicer, and design agents.
- A `designExports` bundle that instantiates those targets as deterministic draft
  3MF/STL/STEP/DXF/CAM setup/nesting/assembly payloads with source text or JSON
  previews, blockers, process-node links, and generated-program links.
- A process plan and structured `processGraph` across 3D printers,
  vertical/horizontal mills, routers, laser, waterjet, plasma/sheet cutters, and
  lathes when those machine profiles are available. The graph links operations,
  generated programs, sequencing dependencies, assembly interfaces, and release
  gates.
- A `machineSelection` trace for every inferred or requested part, including the
  selected machine, required process class, candidate scores, material/process
  rejection reasons, operation gaps, and fallback warnings.
- A `productionPlan` with quantity-aware batches, setup-repeat counts,
  estimated machine minutes, review gates, release blockers, and unattended-run
  eligibility for each part/machine route.
- A `machineSchedule` with deterministic machine lanes, operation start/end
  windows, setup/run/teardown minutes, process dependency holds, postprocessor
  holds, and operator or automation assignments before machine start.
- Draft machine programs such as Marlin-style FDM printer G-code and slicer job
  sheets, SLA/MSLA resin print-wash-cure job sheets, SLS/MJF-style powder-bed
  print-cooldown-depowder job sheets, ISO/Haas-style vertical milling G-code,
  ISO-style horizontal side-slot/keyway milling G-code, GRBL-style router
  profile programs with tab gates, laser, waterjet, and plasma sheet-cutting
  job sheets with kerf tests and fire/fume gates, Fanuc-style turning G-code, or
  operator-only instructions for unsupported machine kinds.
- Validation and simulation findings plus failure boundaries for heat-up, homing,
  spindle-speed/direction/start/process-stop state, work-offset/datum evidence, additive material/color/tool-change,
  manual-stop, CNC tool-change automation/operator-load/spindle-stop evidence,
  mill/router fixture/hold-down evidence, cutting feed-rate/cut-chart evidence,
  tool-length/probe compensation/cancel state, cutter-compensation offset/cancel state, chip/coolant/dust-collection state, lathe
  chuck/stick-out/runout evidence, part-off catcher/support evidence, tool/turret-change stop state, tool-nose compensation evidence/cancel state, canned drilling/tapping cycle setup/cancel state, declared
  material/machine compatibility, additive slicer profile/support/
  orientation/first-layer evidence, additive thin-wall geometry, printer
  async-nozzle-wait state, async-bed-target re-wait state, nozzle-cooldown/
  reheat state, bed-cooldown/re-wait state, stepper-idle/re-home state,
  extrusion-mode/reset state, post-mode-switch extrusion reset state,
  negative-Z extrusion/Z-offset probe state, filament lot/dry-storage
  conditioning evidence, bed-adhesion, first-layer, fan-timing, resin-handling,
  powder-handling,
  sheet-cutting pierce/kerf/focus/gas/fume/support, waterjet pressure/abrasive-flow, and plasma work-clamp evidence, deep-cut, arc-plane/geometry,
  positioning-mode reset state, additive relative-positioning extrusion state, setup-limit, machine-envelope, inspection, and automation constraints.
- A `resolutionPlan` with ordered release-blocking remediation steps derived
  from failure boundaries, including split/combine, human review, automation,
  and regeneration phases.
- A `machineRelease` report with checklist status, release blockers, generated
  and improved program readiness counts, and the current machine-release state.
- An `executionPlan` preflight that turns machine-release blockers, simulation
  traces, and intervention maps into program runs, checkpoints, stop points,
  unattended-run eligibility, and required human or automation actions before
  machine start.
- A `postprocessPlan` preflight with controller-specific targets, postprocessor
  selection, input/output formats, dry-run gates, blockers, required artifacts,
  and operator signoff requirements before any printer, mill, router, sheet
  cutter, lathe, or manual cell can start.
- A `manufacturingHandoff` package with part-level geometry envelopes, stock
  strategy, datum scheme, fixture/setup plan, inspection gates, release blockers,
  and release gates for downstream CAD/CAM, slicer, or shop-floor review.
- A `qualityPlan` with inspection points, measurement targets, records to
  capture, release gates, and learning observations for MDP/POMDP/neural outcome
  feedback after shop-floor evidence is recorded.
- A `toolingPlan` setup traveler with required tools, workholding, consumables,
  setup checks, automation dependencies, release blockers, and production-batch
  links for each generated part route.
- `improvements` and `improvedPrograms` review drafts for generated and
  submitted instruction streams, with conservative gates inserted before
  machine-ready release.
- Assembly advice with a structured `assemblyGraph` of part nodes, hybrid
  interface edges, join/fit strategies, inspection gates, and sequence steps for
  deciding when parts should be combined into one job or split so tight-tolerance
  features can be machined and inspected separately.
- A learning contract with MDP states, POMDP observations, a structured
  `pomdpBeliefState` with hidden-state probabilities and probe actions, policy
  actions, scored `strategyCandidates`, typed `interventionSignals`, reward terms,
  neural feature names, a deterministic neural-policy sketch, and
  `neuralTrainingCorpus` feature vectors, labels, inference candidates, and
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
  parametric design payloads, design packages, design export bundles, process plans,
  production plans, machine programs, validation reports, boundary summaries,
  resolution plans, intervention maps, execution plans, postprocess plans, POMDP
  belief states, neural training corpora, machine-release reports, manufacturing
  handoffs, quality plans, tooling plans, machine-selection traces, improved instructions,
  assembly plans, process graphs, assembly graphs, and optimizer-shaped MDP
  requests.

Real production use still requires CAD/CAM generation, controller-specific
post-processing, simulation, workholding review, material verification, and
operator sign-off.

## `GET /fabrication/capabilities`

`GET /capabilities` and the gateway-prefixed `GET /fabrication/capabilities`
return the service capability contract before a caller submits work. The payload
includes supported request families, built-in `defaultMachines`, machine classes
for FDM, resin, powder-bed, vertical milling, horizontal milling, routing, laser,
waterjet, plasma, lathe, and manual/special-process work, accepted instruction
kinds, generated artifact families, learning channels, and safety boundary
classes. These capabilities describe draft planning and validation support, not
controller-certified release.

## `GET /fabrication/schema` And `GET /fabrication/examples`

`GET /schema` and `GET /fabrication/schema` return a compact request contract for
planning, instruction analysis, learning observations, compact learning outcomes,
machine profiles, instruction programs, and response highlights. `GET /examples`
and `GET /fabrication/examples` return ready-to-edit JSON examples for a hybrid
printed/milled/turned plan, existing CNC and resin-job instruction analysis,
outcome learning, compact learning outcomes, and a NATS instruction-analysis
envelope.

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
words, missing units or positioning modes, CNC program end while still in `G91` incremental positioning without `G90` reset, CNC subprogram calls, macro variables, conditionals, or jumps before controller dependency review evidence, printer extrusion before heat-up,
after async `M104` nozzle targets without `M109` or verified hotend wait,
after async `M140` bed target changes without `M190` or verified bed wait,
after nozzle cooldown without reheat, after bed cooldown without re-wait, after
stepper idle without re-homing, or before homing, missing `M82`/`M83` extrusion
mode and `G92 E0` reset state before priming, missing filament lot/dry-storage/
dryer/desiccant evidence before first extrusion, missing bed-temperature waits or
re-waits, later `M82`/`M83` extrusion-mode switches without renewed `G92 E`
reset evidence, positive extrusion while `G91` relative axis positioning remains
active without `G90` or coordinate-state verification, positive extrusion below
build-surface Z without measured Z-offset/probe evidence, first-layer adhesion setup, early
part-cooling fan timing, additive material/color/tool-change stops such as `M600`
or multi-tool selection, post-change extrusion without purge/prime/resume evidence, printer pauses before renewed position/extrusion resume evidence, selected-tool extrusion without `M104`/`M109` or hotend temperature evidence, mill/router rapid/feed negative-Z plunges after tool selection without
explicit `G43`/probe/tool-length state or later `M6` tool changes before `G49` cancellation, `G41`/`G42` cutter compensation without
`D` offset or tool radius/diameter evidence or without `G40` cancellation before program end, `M6` tool changes before ATC/magazine/
carousel/operator-loaded evidence or while spindle/process remains active without `M5`/`M05` stop evidence, mill/router/lathe cutting feeds and mill/router rapid negative-Z plunges before probed
datum/touch-off/edge-finder/work-offset evidence, mill/router cutting feeds or rapid negative-Z plunges before
fixture/vise/clamp/vacuum/hold-down/tab evidence, cutting moves before positive
`F` feed-rate, chip-load, feeds-and-speeds, or cut-chart evidence, missing
coolant, air blast, dust collection, chip conveyor, or dry-machining approval
before cutting feed moves or after those systems are stopped, sheet-cutter feed
moves before
pierce/kerf/focus/assist-gas/fume/support evidence, waterjet pump-pressure/abrasive-flow evidence, plasma work-clamp/ground-return evidence, or after assist-gas/fume/abrasive support media is stopped, unsafe canned
drilling/peck/tapping cycles with missing or nonpositive `R` retract planes or motion before `G80` cancellation, mill/router/lathe `M3`/`M4` spindle starts without positive `S` speed evidence or changes direction while active without `M5`/`M05` stop evidence, subtractive feed moves before spindle start or after
explicit `M5`/`M05` process stop, mill/router rapid negative-Z plunges before spindle/process start or after explicit `M5`/`M05` process stop without restart, lathe chuck/collet/tailstock/stick-out/runout
evidence before turning feeds, part-off or cutoff operations without catcher/subspindle/tailstock/stock-support evidence, lathe `T` tool/turret changes while spindle/process remains active without `M5`/`M05` stop evidence, lathe `G41`/`G42` tool-nose compensation without tool-nose radius/geometry/wear offset evidence or without `G40` cancellation before program end, lathe
constant-surface-speed without a spindle cap, threading cycles without feed-per-rev or pitch-synchronization evidence, part-off or
cutoff operations, manual stops, fixture changes, deep negative Z moves, arc
moves before explicit `G17`/`G18`/`G19` plane evidence, with center offsets that do not match the selected plane, or without plane-matched `I`/`J`, `I`/`K`, or `J`/`K` center offsets or `R` radius, missing program ends, declared material
incompatibility with resolved machine profiles, and text-instruction boundaries
where the job needs setup, slicer profile/support/
orientation/first-layer evidence, post-processing, resin IPA/wash/cure/waste
controls, powder cooldown/depowder/recovery controls, sheet-cutting
kerf/fire/fume checks, assembly, splitting, or operator intervention. Improved
drafts are still marked `machineReady=false`; they are normalization aids for
review, motion-envelope simulation, and controller-specific postprocessing.
`resolutionPlan` converts those boundaries into ordered remediation steps before
a human or downstream agent attempts machine-ready release.

Machine-code planning and analysis also run a bounded coordinate-envelope
simulation over `G0`/`G1` endpoints, rotary/index `A`/`B`/`C` axis words, and
conservative `G2`/`G3` arc sweeps. Rotary axes are retained in `axisExtents` with
degree units and emit `simulated-rotary-index-review` findings plus
`rotary-index-boundary` review gates for clamp, fixture, clearance, and re-probe
evidence. When a submitted or generated toolpath exceeds the selected machine
`workEnvelopeMm`, the service emits
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
`resolution-plan`, `analysis-resolution-plan`, `intervention-map`, or
`analysis-intervention-map` artifacts. The intervention maps expose
`interventionMap` links for human intervention points, split/combine decisions,
automation paths, program boundary traces, and machine-failure risk scores back
to program IDs and process nodes
when process graph context exists.

Plan responses also include `assembly.assemblyGraph`; the retained
`parametric-design` and `assembly-plan` artifacts carry the same graph so
external CAD/CAM or learning workers can connect generated parts, manufacturing
methods, join interfaces, dry-fit/metrology gates, and assembly sequence steps.
The response, retained `design-package`, retained `parametric-design`, and
`mdp-request` artifacts include `designPackage` export targets, coordinate
frames, model intent, review gates, and export blockers so CAD/CAM, slicer, mesh,
and assembly agents can regenerate neutral 3MF, STL, STEP, DXF, CAM setup JSON,
and assembly JSON/STEP deliverables from the same planning state.
The response, retained `design-export-bundle`, retained `parametric-design`, and
`mdp-request` artifacts include `designExports` generated design export payloads
for CAD/CAM, slicer, mesh review, sheet nesting, and assembly agents; individual
`generated-design-export` artifacts expose draft 3MF/STL/STEP/DXF/CAM setup JSON
content with blockers, review gates, process-node links, and generated-program
links.
The response, retained `parametric-design`, retained `manufacturing-handoff`, and
`mdp-request` artifacts also include `manufacturingHandoff` so downstream
CAD/CAM, slicer, fixture, and learning workers can connect each part to its
geometry primitive, stock and datum assumptions, fixture strategy, draft program,
inspection gates, and machine-release blockers.
The response, retained `production-plan`, retained `parametric-design`, and
`mdp-request` artifacts include `productionPlan` quantity-aware batch data so
schedulers can compare batch counts, setup repeats, estimated machine minutes,
review gates, blockers, and unattended-run eligibility.
The response, retained `machine-schedule`, retained `parametric-design`, and
`mdp-request` artifacts include `machineSchedule` machine-lane utilization,
operation windows, dependency holds, postprocessor holds, and operator or
automation start gates so resource sequencers can see where generated work
cannot enter a printer, mill, router, sheet cutter, lathe, or manual cell.
The response, retained `quality-plan`, retained `parametric-design`, and
`mdp-request` artifacts include `qualityPlan` inspection points, measurement
targets, records to capture, release gates, and learning observations so
inspection, metrology, and learning workers can consume quality evidence without
reparsing shop-floor handoff prose.
The response, retained `tooling-plan`, retained `parametric-design`, and
`mdp-request` artifacts include `toolingPlan` setup-traveler requirements so
operators and downstream agents can review required tools, workholding,
consumables, setup checks, automation dependencies, production-batch links, and
tooling release blockers before machine-ready release.
The response plus retained `machine-selection`, `parametric-design`, and
`mdp-request` artifacts include `machineSelection` candidate scores and selected
machine reasons so learning workers can compare alternate printers, mills,
routers, sheet cutters, and lathes without rerunning the planner.
Plan responses and the retained `process-graph`, `parametric-design`, and
`mdp-request` artifacts include `processGraph` nodes, dependencies, and release
gates so downstream agents can reason over operation order, generated programs,
assembly-interface dependencies, and validation gates without reparsing prose.
The response, retained `execution-plan`, retained `parametric-design`, and
`mdp-request` artifacts include `executionPlan` program runs, release
checkpoints, execution stop points, unattended-run eligibility, and learning
observations so machine and policy workers can see exactly where a job cannot
continue without intervention, automation, split/combine work, or regeneration.
The response, retained `postprocess-plan`, retained `analysis-postprocess-plan`,
retained `parametric-design`, and `mdp-request` artifacts include
`postprocessPlan` controller-specific targets, postprocessor selection,
input/output formats, dry-run or simulation gates, release blockers, required
output artifacts, and operator signoff requirements so slicer/CAM/controller
workers know what must be produced before machine start.

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
- An `outcomeRemediation` plan with root causes, corrective actions, retry
  strategy, human-review status, and learning signals for failed, scrapped, or
  intervention-heavy fabrication attempts.
- A policy snapshot summarizing retained method preferences, assembly
  preferences, and material-specific `remediationRisks` from failed or negative
  fabrication evidence.

Learning outcomes are also recorded as job artifacts: `outcome-learning-event`,
`reward-signal`, `mdp-experience`, `outcome-remediation-plan`,
`pomdp-observations`, and `neural-example`.
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
and recent neural training examples are carried into the returned learning plan
and `neuralTrainingCorpus`.
Failed or negative outcomes are retained as material-specific
`remediationRisks` keyed by method and material; matching future plans surface
`learned-remediation-risk:*` POMDP observations, review/avoid policy actions
such as `avoid-learned-risk-milling-petg`, and remediation examples so the
MDP/POMDP worker can revise programs, split/combine choices, tooling, and
quality gates before retry.
The plan also includes scored `strategyCandidates` such as selected hybrid,
additive consolidation, machined datum-finish, and split-for-inspection options.
These candidates carry methods, machine kinds, estimated time, intervention
counts, boundary counts, scores, and rationale so the MDP/POMDP optimizer can
compare alternate make strategies instead of only seeing the selected route.
`pomdpBeliefState` converts boundary, automation, and caller observations into
machine-failure, human-intervention, split/combine, automation-gap, and
program-valid hidden-state probabilities with observation likelihoods and
recommended probe actions before release. A `neuralPolicy` sketch with a
normalized feature vector, hidden activations, and bounded action scores lets an
external neural model train from the same state or replace the local scoring
head. `neuralTrainingCorpus` carries per-part generated examples,
policy-memory examples, bounded labels, and strategy inference candidates aligned
to `neuralFeatures`. `interventionSignals` expose automation requirements and ordered
`resolutionPlan` steps as learnable actions, observations, next states, and
reward adjustments. The optimizer-shaped `mdp-request` artifact includes
`strategyCandidates`, `interventionSignals`, `pomdpBeliefState`,
`neuralTrainingCorpus`,
`designPackage`, `designExports`, `productionPlan`, `machineSchedule`, `machineSelection`,
`manufacturingHandoff`, `qualityPlan`, `toolingPlan`, `interventionMap`,
`executionPlan`, `postprocessPlan`, `automationRequirements`, `resolutionPlan`,
and `machineRelease` so external MDP/POMDP workers can learn from the same
boundary evidence, design export state, batch-planning state, machine-choice
alternatives, machine-schedule state, quality evidence targets, tooling/setup
requirements, intervention paths, postprocessor gates, and CAD/CAM handoff
assumptions.

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
  `design-package`, `design-export-bundle`, `production-plan`, `machine-schedule`, `machine-selection`, `process-graph`,
  `manufacturing-handoff`, `quality-plan`, `tooling-plan`, `machine-release`,
  `execution-plan`, `postprocess-plan`, `boundary-summary`, `intervention-map`, `simulation-report`, `learning-plan`,
  `pomdp-belief-state`, `neural-training-corpus`,
  `mdp-request`, a `generated-design-export`, a `program-*` generated machine program, or an
  `improved-program-*` instruction rewrite, plus instruction-analysis artifacts such as
  `analysis-boundary-summary`, `analysis-intervention-map`,
  `analysis-machine-release`, `analysis-execution-plan`, `analysis-postprocess-plan`,
  `analysis-simulation-report`, and learning artifacts such as `reward-signal`,
  `mdp-experience`, `outcome-remediation-plan`, `pomdp-observations`, and
  `neural-example`. `design-package`, `parametric-design`, and `mdp-request`
  include `designPackage` export targets, coordinate frames, model intent,
  neutral CAD/mesh/profile formats, assembly exports, review gates, and export
  blockers; `design-export-bundle`, `generated-design-export`,
  `parametric-design`, and `mdp-request` include `designExports` generated design
  export payloads, source previews, media types, blockers, and generated
  program/process-node links; `parametric-design` and `assembly-plan` include
  `assemblyGraph` nodes, interfaces, and sequence gates; `parametric-design`,
  `production-plan`, and `mdp-request` include `productionPlan` batch counts,
  setup repeats, estimated machine minutes, review gates, release blockers, and
  unattended-run eligibility; `machine-schedule`, `parametric-design`, and
  `mdp-request` include `machineSchedule` machine lanes, operation windows,
  dependency holds, postprocessor holds, and operator/automation start gates;
  `parametric-design`,
  `machine-selection`, and `mdp-request` include `machineSelection` candidate
  scoring, selected-machine reasons, and rejection/review status for each part;
  `parametric-design`,
  `manufacturing-handoff`, and `mdp-request` include `manufacturingHandoff`
  part-level stock, datum, fixture, program-link, inspection, and release-blocker
  data; `quality-plan` and `mdp-request` include `qualityPlan` inspection
  points, measurement targets, records to capture, release gates, and learning
  observations; `tooling-plan` and `mdp-request` include `toolingPlan` required
  tools, workholding, consumables, setup checks, automation dependencies,
  production-batch links, and release blockers; `intervention-map`,
  `analysis-intervention-map`, and `mdp-request` include `interventionMap`
  human-intervention points, split/combine decisions, automation paths, program
  boundary traces, learning observations, and machine-failure risk
  scores; `execution-plan`, `analysis-execution-plan`,
  `parametric-design`, and `mdp-request` include `executionPlan` program runs,
  checkpoints, execution stop points, unattended-run eligibility, and required
  intervention or automation actions;
  `postprocess-plan`, `analysis-postprocess-plan`, `parametric-design`, and
  `mdp-request` include `postprocessPlan` controller targets, postprocessor
  choices, input/output formats, dry-run evidence gates, blockers, required
  artifacts, and operator signoff requirements;
  `pomdp-belief-state`, `parametric-design`, and `mdp-request` include
  `pomdpBeliefState` hidden-state probabilities, observation likelihoods, and
  recommended probe actions for uncertain machine-failure, intervention,
  split/combine, automation, and program-valid states; `learning-plan`,
  `neural-training-corpus`, and `mdp-request` include `neuralTrainingCorpus`
  normalized training examples, feature vectors, labels, and strategy inference
  candidates;
  `outcome-remediation-plan` includes `outcomeRemediation` root causes,
  corrective actions, retry strategy, and learning signals from observed
  fabrication outcomes; `process-graph`, and
  `mdp-request` include `processGraph` operation nodes, dependencies, and
  release gates. `parametric-design` also embeds `designPackage`, `designExports`,
  `executionPlan`, `postprocessPlan`, `pomdpBeliefState`, `machineRelease`,
  `manufacturingHandoff`, `productionPlan`, `machineSchedule`, `qualityPlan`, and
  `toolingPlan` for one-payload handoff review.

## Local Build

```bash
cd remote/deployments/fabrication-server-rs
cargo test
cargo run --release
```

The default local port is `8113`; set `PORT` to override it.
