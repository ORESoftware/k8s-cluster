# `remote/deployments/fabrication-server-rs`

Rust fabrication planning service for additive including large-format pellet/FGF,
subtractive, turning, mill-turn/swiss-turning, and hybrid machine workflows.

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
optional part decomposition, optional CAD/model/slicer design inputs, optional
existing instruction streams, and optional learning hints. It returns:

Submitted `existingInstructions` are analyzed beside generated drafts. When the
request declares a material, those submitted programs are also checked against
resolved machine profile material lists before the plan is marked OK.
Submitted `designInputs` are classified as native CAD, cloud CAD, open/scripted
CAD, organic model, neutral geometry, lightweight CAD/PMI exchange, CAD-kernel
exchange, color/scan mesh, 2D sheet/profile CAD, or slicer project evidence
before any downstream worker treats them as releasable geometry. Each entry must
carry a source identity field (`fileName`, `sourceUri`, `format`, or `sourceSystem`);
`role` and `notes` are supplemental only. Source URIs are stored without
userinfo, query strings, or fragments, and ambiguous native extensions such as
bare `.prt` stay release-blocked until source-system or neutral-export evidence
is supplied.

- A normalized design summary with inferred additive, milling, turning, or
  special-process parts.
- A `designPackage` with per-part parametric primitives, coordinate frames,
  neutral export targets such as 3MF, STL, STEP, DXF, CAM setup JSON, and
  assembly graph exports for downstream CAD/CAM, slicer, and design agents.
- A `designExports` bundle that instantiates those targets as deterministic draft
  3MF/STL/STEP/DXF/CAM setup/nesting/assembly payloads with source text or JSON
  previews, blockers, process-node links, and generated-program links.
- A `designInputReview` that recognizes Creo/Pro/ENGINEER, SOLIDWORKS, Fusion,
  Siemens NX, CATIA, Onshape, FreeCAD, OpenSCAD, Blender, ZBrush, STEP/IGES,
  JT lightweight CAD/PMI, 3MF, STL, OBJ, PLY/VRML/glTF/AMF color or scan mesh/package inputs,
  Parasolid/ACIS kernel files, DXF/DWG sheet-profile drawings, and
  PrusaSlicer/OrcaSlicer/Cura/Bambu Studio project sources while retaining
  translator, topology, scale, PMI/tessellation, kernel-version/body-count,
  color/material/texture, layer/kerf/revision, slicer-profile, and release blockers.
  Its `conversionPlan` lists per-input CAD/model/slicer conversion worker lanes,
  design-conversion NATS request/result subjects, preferred neutral exports,
  required evidence, review gates, and machine-release blockers.
- A process plan and structured `processGraph` across 3D printers,
  vertical/5-axis/4th-axis/horizontal mills, routers, laser, waterjet, plasma, wire EDM/sheet cutters,
  sinker/ram EDM cells, robotic assembly/joining cells,
  mill-turn/swiss-turning centers, and lathes when those machine profiles are
  available. The graph links operations,
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
  sheets, large-format pellet/FGF job sheets with pellet lot, drying/moisture,
  hopper/purge, bead/thermal/cooling, gantry-clearance, warpage, and trim-allowance
  gates, SLA/MSLA resin print-wash-cure job sheets, PolyJet/material-jetting
  photopolymer job sheets with cartridge, channel-map, printhead, support-removal,
  UV, and color/material inspection gates, continuous-fiber composite
  matrix/fiber-layup job sheets with fiber orientation, cutter, spool, coupon, and
  delamination gates, SLS/MJF-style powder-bed
  print-cooldown-depowder job sheets, DMLS/SLM/LPBF metal powder-bed fusion job
  sheets with inert-gas/recoater/stress-relief/plate-removal gates, DED/WAAM
  directed-energy deposition job sheets with feedstock, bead-path, shielding-gas,
  melt-pool, interpass, NDE/coupon, and finish-machining allowance gates, binder-jet
  green-part cure/depowder/sinter or infiltration job sheets with binder-saturation,
  printhead, green-strength, and shrink-coupon gates, ISO/Haas-style
  vertical milling G-code, ISO-style five-axis TCP/RTCP milling G-code,
  ISO-style 4th-axis rotary-indexed milling G-code with A-axis index,
  brake/clamp, clearance-sweep, and re-probe checkpoints, ISO-style horizontal side-slot/keyway milling G-code, GRBL-style router
  profile programs with tab gates, laser, waterjet, plasma, and wire EDM sheet-cutting
  job sheets with kerf tests, wire-thread/skim-pass/slug-retention gates, and
  fire/fume/dielectric/flushing gates, sinker/ram EDM cavity burn sheets with
  electrode, dielectric/flushing, orbit-finish, depth-stop, and wear-compensation
  gates, robotic assembly-cell job sheets with kit traceability, datum dry-fit,
  robot path/gripper/fixture/vision evidence, press/heat-set/torque/adhesive
  join recipes, cure or clamp timing, and final metrology gates, mill-turn/swiss
  G-code with C/Y-axis live-tooling and subspindle transfer checkpoints,
  Fanuc-style turning G-code with chuck/stick-out/runout, G50/G95, threading,
  part-off catcher/support, coolant shutdown, and turret-stop checkpoints, or
  operator-only instructions for unsupported machine kinds.
- Validation and simulation findings plus failure boundaries for heat-up, homing,
  spindle-speed/direction/start/process-stop state, work-offset/datum evidence,
  G10 fixture/work-offset table write review state, additive material/color/tool-change,
  manual-stop, CNC tool-change automation/operator-load/spindle-stop evidence,
  subtractive text setup/process evidence, mill/router fixture/hold-down evidence, cutting feed-rate/cut-chart evidence,
  tool-life/wear/load-monitor evidence, tool-length/probe compensation/cancel state, probing-cycle setup/feed/recovery state, cutter-compensation offset/cancel state, chip/coolant/dust-collection state, lathe
  chuck/stick-out/runout evidence, mill-turn live-tooling C/Y-axis/polar-interpolation evidence, mill-turn main/sub-spindle transfer evidence, part-off catcher/support evidence including lathe text part-off support evidence, lathe text threading feed-per-rev/pitch-sync evidence, tool/turret-change stop state, tool-nose compensation evidence/cancel state, canned drilling/tapping cycle setup/cancel state, declared
  material/machine compatibility, additive slicer profile/support/
  orientation/first-layer, mesh unit/scale/topology/wall-thickness evidence,
  high-speed kinematic evidence, additive thin-wall geometry, printer
  async-nozzle-wait state, async-bed-target re-wait state, nozzle-cooldown/
  reheat state, bed-cooldown/re-wait state, stepper-idle/re-home state,
  mid-print homing/resume-position state, additive inch-units/slicer conversion state,
  printer coordinate/home-offset state,
  extrusion-mode/reset state, post-mode-switch extrusion reset state,
  negative-Z extrusion/Z-offset probe state, bed-leveling/mesh restore state,
  filament lot/dry-storage
  conditioning evidence, material-capacity/runout evidence,
  extrusion calibration/flow/pressure-advance evidence,
  volumetric-extrusion/M200 state,
  firmware retraction/recover settings evidence,
  printer G2/G3 arc-support evidence,
  high-speed input-shaper/acceleration/volumetric-flow evidence,
  chamber/enclosure/thermal-soak evidence for warp-prone filament,
  bed-adhesion, first-layer, fan-timing, resin exposure/profile/layer/support evidence,
  resin layer/exposure manifest image-hash/checksum and peel/lift/recoat evidence, resin
  vat-capacity/refill evidence, resin-handling/postprocess evidence,
  pellet/FGF pellet-lot/drying/moisture/hopper/purge and bead/screw/melt/cooling/gantry-clearance/warpage/trim-allowance evidence,
  material-jetting cartridge/channel-map/printhead/tray and support-removal/UV/color/material inspection evidence,
  DED/WAAM feedstock/substrate/bead-path/standoff and laser/arc/shielding/interpass/NDE/coupon evidence,
  composite-fiber layup/orientation/load-case and spool/cutter/coupon/continuity evidence,
  powder-bed build profile/powder lot/nesting evidence, powder-handling/cooldown-depowder evidence,
  metal powder-bed fusion alloy-lot/oxygen/recoater/stress-relief/plate-removal evidence,
  binder-jet binder-lot/saturation/printhead/green-strength and cure/debind/sinter/infiltration/shrink-compensation evidence,
  powder-bed recoater clearance/thermal spacing/cooldown evidence,
  assembly fit/metrology/datum/torque/cure evidence, assembly-cell
  robot-path/gripper/fixture/vision/interlock evidence, assembly-cell
  press/heat-set/torque/adhesive/cure/final-metrology evidence,
  part-separation cut-path/fixture/kerf/deburr/traceability/final-inspection evidence,
  precision tolerance/surface-finish metrology evidence,
  unattended/batch monitoring and recovery evidence,
  thermal postprocess temperature/fixture/cooldown evidence,
  surface/chemical finishing media/masking/PPE/waste evidence,
  indexed setup clamp/index/clearance/re-probe evidence,
  sheet-cutting material/thickness/cut-chart/recipe evidence, pierce/kerf/focus/gas/fume/support, retained-tab/microjoint/part-release evidence, waterjet pressure/abrasive-flow, plasma work-clamp evidence, wire EDM start-hole/thread/tension/dielectric/flushing/slug-retention/skim-pass evidence plus profile/skim-cut setup-order evidence, and sinker EDM electrode/dielectric/depth/wear/orbit-finish/recast release-gate evidence,
  deep-cut, arc-plane/geometry,
  coordinate transform rotation/scaling/mirroring review and cancel state,
  G92 work-coordinate offset review and cancel state, inverse-time feed review and G94 cancel state, G43.4/G234 tool-center-point review and G49 cancel state,
  units-mode change/conversion review state, dwell-duration state,
  positioning-mode reset state, additive relative-positioning extrusion state, setup-limit, machine-envelope, inspection, and automation constraints.
- A `resolutionPlan` with ordered release-blocking remediation steps derived
  from failure boundaries, including split/combine, human review, automation,
  and regeneration phases.
- A `machineRelease` report with checklist status, release blockers, generated
  and improved program readiness counts, release-probe blockers from
  `releaseProbePlan`, and the current machine-release state.
- An `executionPlan` preflight that turns machine-release blockers, simulation
  traces, and intervention maps into program runs, checkpoints, stop points,
  unattended-run eligibility, and required human or automation actions before
  machine start.
- A `postprocessPlan` preflight with controller-specific targets, postprocessor
  selection, input/output formats, dry-run gates, blockers, required artifacts,
  and operator signoff requirements before any printer, mill, mill-turn center,
  router, sheet cutter, lathe, robotic assembly cell, or manual cell can start.
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
  `pomdpBeliefState` with hidden-state probabilities and probe actions, a
  `releaseProbePlan` of priority evidence probes required before release, policy
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
  belief states, release probe plans, neural training corpora, machine-release reports, manufacturing
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
for FDM, large-format pellet/FGF, resin, material jetting, directed-energy deposition/WAAM, continuous-fiber composite, binder jet, polymer powder-bed, metal PBF, vertical milling, five-axis milling, rotary-indexed milling, horizontal milling,
mill-turn/swiss-turning, routing, laser,
waterjet, plasma, robotic assembly/joining, lathe, and manual/special-process
work, accepted instruction kinds including slicer, pellet-FGF, SLA/resin,
material-jetting, DED/WAAM, composite-fiber, binder-jet, SLS/powder, metal-PBF,
mill-turn, lathe/turning, indexed-mill, assembly-cell, part-separation, laser/waterjet/plasma,
wire-EDM, and sinker-EDM job sheets, design input format
families, generated artifact families, learning
channels, bounded `profileEvidence` buckets for submitted machine profiles, and
safety boundary classes. These capabilities describe draft planning and
validation support, not controller-certified release.

## `GET /fabrication/schema` And `GET /fabrication/examples`

`GET /schema` and `GET /fabrication/schema` return a compact request contract for
planning, instruction analysis, learning observations, compact learning outcomes,
machine profiles, optional machine-profile evidence, instruction programs, and
response highlights. `GET /examples` and `GET /fabrication/examples` return
ready-to-edit JSON examples for a hybrid printed/milled/turned plan with
calibration/tool/fixture/material/process evidence, existing CNC and resin-job
instruction analysis, outcome learning, compact learning outcomes, and a NATS
instruction-analysis envelope.

Submitted `profileEvidence.blockers` are promoted into validation findings,
`machine-profile-blocker` failure boundaries, resolution steps, machine-release
blockers, production release blockers, and tooling/handoff review gates until
fresh operator, controller, calibration, material, fixture, or process-support
evidence clears them. When several submitted machines can satisfy the same part,
selection prefers compatible machines with no retained profile blockers and
keeps blocked alternates visible in `machineSelection` as
`rejected-profile-blocker` candidates.

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
      "operations": ["additive-print"],
      "profileEvidence": {
        "calibration": ["bed-level-current", "nozzle-offset-current"],
        "tools": ["0.4mm-nozzle-loaded"],
        "fixtures": ["build-plate-clean"],
        "materials": ["PETG-loaded", "filament-drying-required"],
        "process": ["purge-line-required"],
        "blockers": ["material-conditioning-required"]
      }
    },
    {
      "id": "tm1p",
      "kind": "vertical-mill",
      "controller": "haas-gcode",
      "materials": ["aluminum", "brass", "plastic"],
      "workEnvelopeMm": [760, 300, 400],
      "axes": 3,
      "operations": ["face", "pocket", "drill", "contour"],
      "profileEvidence": {
        "calibration": ["g54-work-offset-current", "tool-length-offset-current"],
        "tools": ["t06-6mm-endmill-loaded"],
        "fixtures": ["vise-loaded", "operator-fixture-photo-required"],
        "process": ["coolant-ready", "chip-evacuation-ready"],
        "release": ["dry-run-required"],
        "blockers": ["fixture-proof-required"]
      }
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
  "designInputs": [
    {
      "id": "native-solidworks-body",
      "fileName": "fixture-body.SLDPRT",
      "format": "SLDPRT",
      "sourceSystem": "SOLIDWORKS",
      "role": "editable source CAD"
    },
    {
      "id": "creo-assembly",
      "fileName": "threaded-insert.asm",
      "format": "Pro/ENGINEER assembly",
      "sourceSystem": "PTC Creo",
      "role": "supplier assembly reference"
    },
    {
      "id": "slicer-project",
      "fileName": "fixture-body.3mf",
      "format": "3MF",
      "sourceSystem": "PrusaSlicer",
      "role": "slicer project evidence"
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
FDM printer, SLA resin printer, material-jetting printer, continuous-fiber composite printer, SLS powder-bed printer, DED/WAAM directed-energy deposition cell, metal PBF printer, binder jet printer, vertical
mill,
five-axis mill, rotary-indexer mill, horizontal mill, CNC router, laser cutter, waterjet cutter, plasma cutter, wire
EDM cutter, sinker EDM cell, robotic assembly cell, and lathe. If `parts` is omitted, the planner infers
a first decomposition from the objective, material, and tolerance, including
resin-print, material-jetting-print, directed-energy-deposition, composite-fiber-print, binder-jet-print, polymer powder-bed-print, metal PBF-print, five-axis-milled impellers/undercuts,
4th-axis indexed multi-face milling, horizontal-milled side slots/keyways, laser,
waterjet, plasma, wire EDM, sinker EDM cavity burns, assembly-joining/fit-up
steps, and kerf-controlled
sheet-cut profiles, and routed sheet/profile parts for wood,
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
including positive work-envelope values, unique IDs, nonzero axis counts, and
bounded non-secret `profileEvidence` lists for calibration, tools, fixtures,
materials, process support, maintenance, release evidence, and retained blockers.

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
words, missing units or positioning modes, late or mid-program `G20`/`G21` unit-mode changes after motion without conversion review, CNC program end while still in `G91` incremental positioning without `G90` reset, CNC inverse-time `G93` feed motion without timing review or program end before `G94` cancel, `G43.4`/`G234` tool-center-point mode before rotary/linear motion or program end without TCP kinematic review and `G49` cancellation, `G92` work-coordinate offsets before motion or program end without temporary-offset review and `G92.1`/`G92.2` cancellation, `G10 L2`/`G10 L20` fixture/work-offset table writes without controller offset-table backup or review evidence, CNC subprogram calls, macro variables, conditionals, or jumps before controller dependency review evidence, printer extrusion before heat-up,
after async `M104` nozzle targets without `M109` or verified hotend wait,
after async `M140` bed target changes without `M190` or verified bed wait,
after nozzle cooldown without reheat, after bed cooldown without re-wait, after
stepper idle without re-homing, after mid-print `G28` homing without safe-park,
Z-hop, or resume-position evidence, after `G20` inch-mode selection without slicer/printer
unit-conversion evidence, after `M206`/`G92 X/Y/Z` printer coordinate/home
offsets without offset-probe or dry-run evidence, or before homing, missing
`M82`/`M83` extrusion mode and `G92 E0` reset state before priming, firmware `G10`/`G11` retract/unretract before `M207`/`M208`/`M209` or equivalent retraction settings evidence, missing spool-weight/remaining-filament/runout-sensor evidence before long extrusion, missing filament lot/dry-storage/
dryer/desiccant evidence before first extrusion, missing extrusion
calibration/flow/pressure-advance evidence before first extrusion, missing
chamber/enclosure/thermal-soak evidence before first extrusion for ABS/ASA/PC/nylon,
missing bed-temperature waits or
re-waits, later `M82`/`M83` extrusion-mode switches without renewed `G92 E`
reset evidence, positive extrusion while `G91` relative axis positioning remains
active without `G90` or coordinate-state verification, positive extrusion below
build-surface Z without measured Z-offset/probe evidence, positive extrusion after
`M420 S0` or bed-leveling/mesh-compensation disable without `M420 S1`, `G29`, or
equivalent bed-mesh/Z-offset verification, first-layer adhesion setup, early
part-cooling fan timing, additive material/color/tool-change stops such as `M600`
or multi-tool selection, post-change extrusion without purge/prime/resume evidence, printer pauses before renewed position/extrusion resume evidence, selected-tool extrusion without `M104`/`M109` or hotend temperature evidence, printer `G2`/`G3` arcs without firmware/slicer arc-support evidence, `M200` volumetric extrusion before filament-diameter/slicer volumetric E-unit evidence, high-speed FDM extrusion without input-shaper/acceleration/volumetric-flow evidence, mill/router rapid/feed negative-Z plunges after tool selection without
explicit `G43`/probe/tool-length state or later `M6` tool changes before `G49` cancellation, `G41`/`G42` cutter compensation without
`D` offset or tool radius/diameter evidence or without `G40` cancellation before program end, `M6` tool changes before ATC/magazine/
carousel/operator-loaded evidence or while spindle/process remains active without `M5`/`M05` stop evidence, mill/router/lathe cutting feeds and mill/router rapid negative-Z plunges before probed
datum/touch-off/edge-finder/work-offset evidence, mill/router cutting feeds or rapid negative-Z plunges before
fixture/vise/clamp/vacuum/hold-down/tab evidence, cutting moves before positive
`F` feed-rate, chip-load, feeds-and-speeds, or cut-chart evidence, `G31`/`G38.x` probing cycles before touch-probe calibration, skip/contact input, safe-feed, and retract/recovery evidence, long mill/router/lathe cutting feeds before tool-life, wear-inspection, fresh-edge, or load-monitor evidence, missing
coolant, air blast, dust collection, chip conveyor, or dry-machining approval
before cutting feed moves or after those systems are stopped, sheet-cutter feed
moves before
pierce/kerf/focus/assist-gas/fume/support evidence, outside-profile release cuts before retained-tab/bridge/microjoint/catcher/tip-up evidence, waterjet pump-pressure/abrasive-flow evidence, plasma work-clamp/ground-return evidence, or after assist-gas/fume/abrasive support media is stopped, unsafe canned
drilling/peck/tapping cycles with missing or nonpositive `R` retract planes or motion before `G80` cancellation, mill/router/lathe `M3`/`M4` spindle starts without positive `S` speed evidence or changes direction while active without `M5`/`M05` stop evidence, subtractive feed moves before spindle start or after
explicit `M5`/`M05` process stop, CNC/subtractive program end before explicit
`M5`/`M05` process stop or `M9`/`M09` coolant/support-media shutdown, mill/router rapid negative-Z plunges before spindle/process start or after explicit `M5`/`M05` process stop without restart, lathe chuck/collet/tailstock/stick-out/runout
evidence before turning feeds, part-off or cutoff operations without catcher/subspindle/tailstock/stock-support evidence, lathe `T` tool/turret changes while spindle/process remains active without `M5`/`M05` stop evidence, lathe `G41`/`G42` tool-nose compensation without tool-nose radius/geometry/wear offset evidence or without `G40` cancellation before program end, lathe
constant-surface-speed without a spindle cap, threading cycles without feed-per-rev or pitch-synchronization evidence, part-off or
cutoff operations, manual stops, fixture changes, deep negative Z moves, arc
moves before explicit `G17`/`G18`/`G19` plane evidence, with center offsets that do not match the selected plane, or without plane-matched `I`/`J`, `I`/`K`, or `J`/`K` center offsets or `R` radius, or mill/router programs ending in `G18`/`G19` without `G17` plane restoration, missing program ends, declared material
incompatibility with resolved machine profiles, and text-instruction boundaries
where the job needs setup, subtractive text setup/process evidence for
workholding/datum/tool-length and spindle/feed/coolant/kerf/pierce/cut-chart
controls, slicer profile/support/orientation/first-layer evidence, missing slicer mesh unit/scale/watertight/manifold/normals/wall-thickness evidence for STL/3MF/OBJ/model inputs, slicer high-speed input-shaper/acceleration/volumetric-flow evidence, post-processing, missing pellet/FGF pellet-lot/drying/moisture/hopper/purge/nozzle evidence, missing pellet/FGF bead width/layer height/screw/melt/cooling/gantry-clearance/warpage/trim evidence, missing material-jetting cartridge/material-channel/printhead/tray evidence, missing material-jetting support-removal/UV/color/material inspection evidence, missing DED/WAAM feedstock/substrate/bead-path/standoff/machining-allowance evidence, missing DED/WAAM energy/shielding/melt-pool/interpass/NDE/coupon evidence, missing composite-fiber layup/orientation/load-case evidence, missing composite-fiber spool/cutter/matrix/coupon/continuity inspection evidence, missing resin exposure/profile/layer/support/build-plate evidence, missing resin layer/exposure manifest image hash/checksum or peel/lift/recoat evidence, missing resin vat-volume/level/refill evidence for large resin jobs, resin IPA/wash/cure/drain/PPE/
waste controls or missing resin postprocess evidence, powder
build profile/powder lot/nesting controls or missing powder-bed build/profile evidence,
cooldown/depowder/recovery controls or missing powder-bed handling evidence, missing
metal-PBF alloy-lot/oxygen/recoater/stress-relief/plate-removal evidence, missing
powder-bed recoater clearance/thermal spacing/cooldown evidence, missing binder-jet binder/saturation/printhead/green-strength evidence, missing binder-jet cure/debind/sinter/infiltration/shrink-compensation evidence, assembly
dry-fit/metrology/datum/torque/cure controls or missing assembly fit/metrology evidence, missing assembly-cell robot-path/gripper/fixture/vision/interlock evidence, missing assembly-cell press/heat-set/torque/adhesive/cure/final-metrology evidence, missing part-separation cut-path/fixture/kerf/heat/deburr/traceability/final-inspection evidence, missing precision tolerance/surface-finish metrology evidence, missing unattended/batch monitoring and recovery evidence, missing thermal postprocess temperature/furnace/atmosphere/cooldown/quench/inspection evidence, missing surface/chemical finishing media/masking/PPE/waste/thickness/inspection evidence, missing indexed setup clamp/brake/index-angle/clearance/re-probe evidence, unreviewed `G51` scaling/mirroring or `G68` coordinate rotation and missing `G50.1`/`G69` transform cancellation, `G43.4`/`G234` tool-center-point mode before rotary/linear motion or program end without TCP kinematic review and `G49` cancellation, `G92` work-coordinate offsets before motion or program end without temporary-offset review and `G92.1`/`G92.2` cancellation, `G10 L2`/`G10 L20` fixture/work-offset table writes without controller offset-table backup or review evidence, late or mid-program `G20`/`G21` unit-mode changes after motion without conversion review, sheet-cutting
kerf/fire/fume checks or missing sheet-cutting material/thickness/cut-chart recipe evidence, missing wire EDM start-hole/threading/slug-retention/dielectric/flushing/skim-pass evidence, wire EDM profile/skim cuts before start-hole, wire-threading, guide/tension, conductive workholding, or slug-retention setup evidence, missing sinker EDM electrode/dielectric/flushing/debris-removal/depth/orbit-finish/recast evidence, missing mill-turn live-tooling C-axis/Y-axis/polar-interpolation evidence, missing mill-turn subspindle pickup/clamp/sync/pull-force/transfer-clearance evidence, `G4`/`G04` dwell commands without positive `P`/`S`/`X`/`U` duration or operator-timed dwell review, lathe text threading feed-per-rev/pitch/spindle-encoder evidence, lathe text part-off catcher/subspindle/tailstock/stock-support evidence, assembly, splitting, or operator intervention. Improved
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
Instruction-analysis responses also include a `learning` plan derived from the
submitted programs, boundary evidence, improvements, and release probes. The
retained `analysis-learning-plan`, `analysis-pomdp-belief-state`,
`analysis-release-probe-plan`, `analysis-neural-training-corpus`, and
`analysis-mdp-request` artifacts let MDP/POMDP and neural workers learn from
imported CNC, slicer, printer, and text instruction streams without requiring a
new generated design plan first.

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
The response, retained `design-input-review`, retained `parametric-design`, and
`mdp-request` artifacts include `designInputReview` source classification,
supported format catalog entries, preferred neutral exports, slicer targets,
per-input `conversionPlan` worker handoffs to
`dd.remote.fabrication.design.conversion.requests`, and translator/topology/profile
blockers plus PMI/tessellation, kernel-version/body-count, color/material/texture,
and layer/kerf/revision gates for CAD/model/slicer, lightweight CAD/PMI,
CAD-kernel, color/scan mesh, and 2D sheet-profile intake.
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
cannot enter a printer, mill, mill-turn center, router, sheet cutter, lathe, or
manual cell.
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
routers, mill-turn centers, wire EDM, sinker EDM, other sheet/special-process
cells, and lathes without rerunning the planner.
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
- A policy snapshot summarizing retained method preferences, ordered operation
  sequences, machine-kind preferences, assembly preferences, and
  material-specific `remediationRisks` from failed or negative fabrication
  evidence.

Learning outcomes are also recorded as job artifacts: `outcome-learning-event`,
`reward-signal`, `mdp-experience`, `outcome-remediation-plan`,
`pomdp-observations`, and `neural-example`.
`GET /fabrication/learning/policy` and `GET /learning/policy` return the
current bounded in-process policy memory. `POST /learning/outcomes` and
`POST /fabrication/learning/outcomes` accept a compact success/reward record
when callers already have their own training features.

When a policy snapshot has at least two positive samples for a method such as
`additive-print`, `milling`, `five-axis-milling`, `horizontal-milling`, `routing`, `sheet-cutting`,
or `turning`, subsequent `/fabrication/plan` requests without explicit
`preferredMethods` inherit those learned process preferences. Repeated
multi-method successes such as `additive-print+milling` are retained as method
combination preferences; open future requests can be decomposed into learned
hybrid parts before machine selection. Repeated ordered successes such as
`additive-print>milling>turning` are retained as operation-sequence preferences;
open future requests can be decomposed into learned sequence parts in that order
and surfaced as `learned-operation-sequence-preference:*` POMDP observations
plus `prefer-learned-operation-sequence-*` actions. Strong assembly preferences
such as `printed body plus turned insert` are reused as learned hybrid join strategies,
successful `machineKind` samples such as `resin-printer` are retained as
machine-kind preferences and surfaced as
`learned-machine-kind-preference:*` POMDP observations plus
`prefer-learned-machine-kind-*` actions for open future plans, and recent neural
training examples are carried into the returned learning plan and
`neuralTrainingCorpus`.
Failed or negative outcomes are retained as material-specific
`remediationRisks` keyed by method and material; matching future plans surface
`learned-remediation-risk:*` POMDP observations, review/avoid policy actions
such as `avoid-learned-risk-milling-petg`, and remediation examples so the
MDP/POMDP worker can revise programs, split/combine choices, tooling, and
quality gates before retry. Those learned remediation risks also contribute
machine-failure hidden-state evidence and add a
`learned-remediation-risk:review-prior-failure-outcome-before-release`
required-before-release action in `releaseProbePlan`.
The plan also includes scored `strategyCandidates` such as selected hybrid,
additive consolidation, machined datum-finish, and split-for-inspection options.
These candidates carry methods, machine kinds, estimated time, intervention
counts, boundary counts, scores, and rationale so the MDP/POMDP optimizer can
compare alternate make strategies instead of only seeing the selected route.
`pomdpBeliefState` converts boundary, automation, and caller observations into
machine-failure, human-intervention, split/combine, automation-gap, and
program-valid hidden-state probabilities with observation likelihoods and
recommended probe actions before release. `releaseProbePlan` promotes those
POMDP probes into required/recommended/watch priorities, required-before-release
actions, release-blocker flags, and evidence snippets for downstream POMDP
workers or operators, including required review of matching learned
remediation-risk memory before machine release. Planning feeds those release
probes back into `machineRelease` as `release-probe` blockers and checklist
evidence so learned failure memory can hold the top-level machine release. A
`neuralPolicy` sketch with a
normalized feature vector, hidden activations, and bounded action scores lets an
external neural model train from the same state or replace the local scoring
head. `neuralTrainingCorpus` carries per-part generated examples,
policy-memory examples, bounded labels, and strategy inference candidates aligned
to `neuralFeatures`. `interventionSignals` expose automation requirements and ordered
`resolutionPlan` steps as learnable actions, observations, next states, and
reward adjustments. The optimizer-shaped `mdp-request` artifact includes
`strategyCandidates`, `interventionSignals`, `pomdpBeliefState`, `releaseProbePlan`,
`neuralTrainingCorpus`,
`designPackage`, `designExports`, `designInputReview`, `productionPlan`,
`machineSchedule`, `machineSelection`, `manufacturingHandoff`, `qualityPlan`,
`toolingPlan`, `interventionMap`, `executionPlan`, `postprocessPlan`,
`automationRequirements`, `resolutionPlan`, and `machineRelease` so external
MDP/POMDP workers can learn from the same boundary evidence, design export state,
CAD/model/slicer source assumptions, batch-planning state, machine-choice
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
  `design-package`, `design-export-bundle`, `design-input-review`, `production-plan`, `machine-schedule`, `machine-selection`, `process-graph`,
  `manufacturing-handoff`, `quality-plan`, `tooling-plan`, `machine-release`,
  `execution-plan`, `postprocess-plan`, `boundary-summary`, `intervention-map`, `simulation-report`, `learning-plan`,
  `pomdp-belief-state`, `release-probe-plan`, `neural-training-corpus`,
  `mdp-request`, a `generated-design-export`, a `program-*` generated machine program, or an
  `improved-program-*` instruction rewrite, plus instruction-analysis artifacts such as
  `analysis-boundary-summary`, `analysis-intervention-map`,
  `analysis-machine-release`, `analysis-execution-plan`, `analysis-postprocess-plan`,
  `analysis-simulation-report`, `analysis-learning-plan`,
  `analysis-pomdp-belief-state`, `analysis-release-probe-plan`,
  `analysis-neural-training-corpus`, `analysis-mdp-request`, and learning artifacts
  such as `reward-signal`, `mdp-experience`, `outcome-remediation-plan`,
  `pomdp-observations`, and `neural-example`. `design-package`, `parametric-design`, and `mdp-request`
  include `designPackage` export targets, coordinate frames, model intent,
  neutral CAD/mesh/profile formats, assembly exports, review gates, and export
  blockers; `design-export-bundle`, `generated-design-export`,
  `parametric-design`, and `mdp-request` include `designExports` generated design
  export payloads, source previews, media types, blockers, and generated
  program/process-node links; `design-input-review`, `parametric-design`, and
  `mdp-request` include `designInputReview` supported format families, reviewed
  inputs, preferred neutral exports, slicer targets, per-input conversion worker
  lanes, required evidence, review gates, and release blockers;
  `parametric-design` and `assembly-plan` include
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
  artifacts including assembly-kit travelers, robot-path or fixture simulation
  reports, final-fit metrology records, and operator signoff requirements;
  `pomdp-belief-state`, `release-probe-plan`, `parametric-design`, and `mdp-request` include
  `pomdpBeliefState` hidden-state probabilities, observation likelihoods, and
  recommended probe actions for uncertain machine-failure, intervention,
  split/combine, automation, and program-valid states plus `releaseProbePlan`
  priority probes, release-blocker flags, and required-before-release actions; `learning-plan`,
  `neural-training-corpus`, and `mdp-request` include `neuralTrainingCorpus`
  normalized training examples, feature vectors, labels, and strategy inference
  candidates;
  `outcome-remediation-plan` includes `outcomeRemediation` root causes,
  corrective actions, retry strategy, and learning signals from observed
  fabrication outcomes; `process-graph`, and
  `mdp-request` include `processGraph` operation nodes, dependencies, and
  release gates. `parametric-design` also embeds `designPackage`, `designExports`,
  `designInputReview`, `executionPlan`, `postprocessPlan`, `pomdpBeliefState`,
  `releaseProbePlan`,
  `machineRelease`, `manufacturingHandoff`, `productionPlan`, `machineSchedule`,
  `qualityPlan`, and `toolingPlan` for one-payload handoff review.

## Local Build

```bash
cd remote/deployments/fabrication-server-rs
cargo test
cargo run --release
```

The default local port is `8113`; set `PORT` to override it.
