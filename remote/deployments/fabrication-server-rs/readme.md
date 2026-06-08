# `remote/deployments/fabrication-server-rs`

Rust fabrication planning service for additive including large-format pellet/FGF,
robotic/gantry additive cells, and sheet-lamination/LOM/UAM printers, subtractive, turning,
mill-turn/swiss-turning, and hybrid machine workflows.

It exposes:

`GET /` returns the machine-readable service inventory with a `landingPage`
block for the human fabrication overview, a compact `startHere` map to
`/fabrication/landing`, `/fabrication/how-it-works`, capabilities, schema,
examples, and generated API docs. The landing page and JSON how-it-works
overview explain how the service turns fabrication goals into
design/import decisions, machine/material route choices, split/combine plans,
instruction generation or review, validation, release gates, and learned
outcomes.

- `GET /`
- `GET /landing`
- `GET /fabrication/landing`
- `GET /how-it-works`
- `GET /fabrication/how-it-works`
- `GET /healthz`
- `GET /readyz`
- `GET /metrics`
- `GET /capabilities`
- `GET /fabrication/capabilities`
- `GET /machines/catalog`
- `GET /fabrication/machines/catalog`
- `GET /printers/catalog`
- `GET /fabrication/printers/catalog`
- `GET /printers/preflight/catalog`
- `GET /fabrication/printers/preflight/catalog`
- `GET /subtractive/catalog`
- `GET /fabrication/subtractive/catalog`
- `GET /subtractive/preflight/catalog`
- `GET /fabrication/subtractive/preflight/catalog`
- `GET /turning/preflight/catalog`
- `GET /fabrication/turning/preflight/catalog`
- `GET /cleanliness/preflight/catalog`
- `GET /fabrication/cleanliness/preflight/catalog`
- `GET /interfaces/preflight/catalog`
- `GET /fabrication/interfaces/preflight/catalog`
- `GET /cnc/catalog`
- `GET /fabrication/cnc/catalog`
- `GET /cells/catalog`
- `GET /fabrication/cells/catalog`
- `POST /machines/select`
- `POST /fabrication/machines/select`
- `GET /controllers/catalog`
- `GET /fabrication/controllers/catalog`
- `GET /controllers/preflight/catalog`
- `GET /fabrication/controllers/preflight/catalog`
- `POST /controllers/result`
- `POST /fabrication/controllers/result`
- `GET /process/catalog`
- `GET /fabrication/process/catalog`
- `GET /materials/catalog`
- `GET /fabrication/materials/catalog`
- `POST /materials/plan`
- `POST /fabrication/materials/plan`
- `POST /materials/result`
- `POST /fabrication/materials/result`
- `GET /design/formats`
- `GET /fabrication/design/formats`
- `GET /slicers/catalog`
- `GET /fabrication/slicers/catalog`
- `POST /slicers/result`
- `POST /fabrication/slicers/result`
- `GET /mesh-repair/catalog`
- `GET /fabrication/mesh-repair/catalog`
- `POST /mesh-repair/result`
- `POST /fabrication/mesh-repair/result`
- `GET /formats/catalog`
- `GET /fabrication/formats/catalog`
- `GET /design/import/catalog`
- `GET /fabrication/design/import/catalog`
- `GET /design/preflight/catalog`
- `GET /fabrication/design/preflight/catalog`
- `GET /subjects/catalog`
- `GET /fabrication/subjects/catalog`
- `GET /workers/catalog`
- `GET /fabrication/workers/catalog`
- `GET /results/catalog`
- `GET /fabrication/results/catalog`
- `POST /design/import/review`
- `POST /fabrication/design/import/review`
- `POST /design/import/result`
- `POST /fabrication/design/import/result`
- `POST /design/convert/plan`
- `POST /fabrication/design/convert/plan`
- `POST /design/convert/result`
- `POST /fabrication/design/convert/result`
- `GET /design/generation/catalog`
- `GET /fabrication/design/generation/catalog`
- `POST /design/generate`
- `POST /fabrication/design/generate`
- `POST /design/synthesis/result`
- `POST /fabrication/design/synthesis/result`
- `GET /handoff/catalog`
- `GET /fabrication/handoff/catalog`
- `POST /handoff/result`
- `POST /fabrication/handoff/result`
- `GET /instructions/languages`
- `GET /fabrication/instructions/languages`
- `GET /instructions/review-pipeline/catalog`
- `GET /fabrication/instructions/review-pipeline/catalog`
- `GET /instructions/import/catalog`
- `GET /fabrication/instructions/import/catalog`
- `GET /instructions/import/preflight/catalog`
- `GET /fabrication/instructions/import/preflight/catalog`
- `POST /instructions/import/review`
- `POST /fabrication/instructions/import/review`
- `GET /instructions/validation/catalog`
- `GET /fabrication/instructions/validation/catalog`
- `GET /instructions/validation/preflight/catalog`
- `GET /fabrication/instructions/validation/preflight/catalog`
- `GET /instructions/generation/catalog`
- `GET /fabrication/instructions/generation/catalog`
- `GET /instructions/generation/preflight/catalog`
- `GET /fabrication/instructions/generation/preflight/catalog`
- `POST /instructions/generate`
- `POST /fabrication/instructions/generate`
- `POST /instructions/generation/result`
- `POST /fabrication/instructions/generation/result`
- `POST /instructions/review/result`
- `POST /fabrication/instructions/review/result`
- `POST /instructions/validation/result`
- `POST /fabrication/instructions/validation/result`
- `GET /machine-code/catalog`
- `GET /fabrication/machine-code/catalog`
- `GET /machine-code/preflight/catalog`
- `GET /fabrication/machine-code/preflight/catalog`
- `POST /machine-code/generate`
- `POST /fabrication/machine-code/generate`
- `POST /machine-code/result`
- `POST /fabrication/machine-code/result`
- `GET /toolpaths/catalog`
- `GET /fabrication/toolpaths/catalog`
- `POST /toolpaths/plan`
- `POST /fabrication/toolpaths/plan`
- `POST /toolpaths/result`
- `POST /fabrication/toolpaths/result`
- `GET /improvements/catalog`
- `GET /fabrication/improvements/catalog`
- `GET /improvements/preflight/catalog`
- `GET /fabrication/improvements/preflight/catalog`
- `GET /boundaries/catalog`
- `GET /fabrication/boundaries/catalog`
- `GET /boundaries/preflight/catalog`
- `GET /fabrication/boundaries/preflight/catalog`
- `POST /boundaries/result`
- `POST /fabrication/boundaries/result`
- `GET /remediation/catalog`
- `GET /fabrication/remediation/catalog`
- `POST /remediation/plan`
- `POST /fabrication/remediation/plan`
- `POST /remediation/result`
- `POST /fabrication/remediation/result`
- `GET /decomposition/catalog`
- `GET /fabrication/decomposition/catalog`
- `POST /decomposition/plan`
- `POST /fabrication/decomposition/plan`
- `POST /decomposition/result`
- `POST /fabrication/decomposition/result`
- `GET /assembly/catalog`
- `GET /fabrication/assembly/catalog`
- `GET /assembly/preflight/catalog`
- `GET /fabrication/assembly/preflight/catalog`
- `POST /assembly/plan`
- `POST /fabrication/assembly/plan`
- `POST /assembly/result`
- `POST /fabrication/assembly/result`
- `POST /interfaces/result`
- `POST /fabrication/interfaces/result`
- `GET /release/catalog`
- `GET /fabrication/release/catalog`
- `GET /release/preflight/catalog`
- `GET /fabrication/release/preflight/catalog`
- `POST /release/preview`
- `POST /fabrication/release/preview`
- `POST /release/result`
- `POST /fabrication/release/result`
- `GET /execution/preflight/catalog`
- `GET /fabrication/execution/preflight/catalog`
- `POST /execution/plan`
- `POST /fabrication/execution/plan`
- `POST /execution/result`
- `POST /fabrication/execution/result`
- `GET /hybrid/catalog`
- `GET /fabrication/hybrid/catalog`
- `GET /strategy/catalog`
- `GET /fabrication/strategy/catalog`
- `GET /methods/catalog`
- `GET /fabrication/methods/catalog`
- `POST /strategy/recommend`
- `POST /fabrication/strategy/recommend`
- `POST /strategy/result`
- `POST /fabrication/strategy/result`
- `GET /schedule/catalog`
- `GET /fabrication/schedule/catalog`
- `POST /schedule/result`
- `POST /fabrication/schedule/result`
- `GET /simulation/catalog`
- `GET /fabrication/simulation/catalog`
- `GET /simulation/preflight/catalog`
- `GET /fabrication/simulation/preflight/catalog`
- `POST /simulation/run`
- `POST /fabrication/simulation/run`
- `POST /simulation/result`
- `POST /fabrication/simulation/result`
- `GET /quality/catalog`
- `GET /fabrication/quality/catalog`
- `GET /quality/preflight/catalog`
- `GET /fabrication/quality/preflight/catalog`
- `GET /dispositions/catalog`
- `GET /fabrication/dispositions/catalog`
- `POST /dispositions/result`
- `POST /fabrication/dispositions/result`
- `GET /costing/catalog`
- `GET /fabrication/costing/catalog`
- `POST /costing/result`
- `POST /fabrication/costing/result`
- `GET /utilities/catalog`
- `GET /fabrication/utilities/catalog`
- `GET /energy/catalog`
- `GET /fabrication/energy/catalog`
- `POST /energy/result`
- `POST /fabrication/energy/result`
- `POST /utilities/result`
- `POST /fabrication/utilities/result`
- `GET /telemetry/catalog`
- `GET /fabrication/telemetry/catalog`
- `GET /maintenance/catalog`
- `GET /fabrication/maintenance/catalog`
- `POST /maintenance/result`
- `POST /fabrication/maintenance/result`
- `GET /availability/catalog`
- `GET /fabrication/availability/catalog`
- `POST /availability/result`
- `POST /fabrication/availability/result`
- `POST /telemetry/result`
- `POST /fabrication/telemetry/result`
- `POST /quality/plan`
- `POST /fabrication/quality/plan`
- `POST /quality/result`
- `POST /fabrication/quality/result`
- `GET /calibration/catalog`
- `GET /fabrication/calibration/catalog`
- `POST /calibration/plan`
- `POST /fabrication/calibration/plan`
- `POST /calibration/result`
- `POST /fabrication/calibration/result`
- `GET /interventions/catalog`
- `GET /fabrication/interventions/catalog`
- `POST /interventions/result`
- `POST /fabrication/interventions/result`
- `GET /setup/catalog`
- `GET /fabrication/setup/catalog`
- `GET /tooling/catalog`
- `GET /fabrication/tooling/catalog`
- `POST /tooling/result`
- `POST /fabrication/tooling/result`
- `GET /consumables/catalog`
- `GET /fabrication/consumables/catalog`
- `POST /consumables/result`
- `POST /fabrication/consumables/result`
- `GET /workholding/catalog`
- `GET /fabrication/workholding/catalog`
- `GET /workholding/preflight/catalog`
- `GET /fabrication/workholding/preflight/catalog`
- `POST /workholding/result`
- `POST /fabrication/workholding/result`
- `GET /nesting/catalog`
- `GET /fabrication/nesting/catalog`
- `POST /nesting/result`
- `POST /fabrication/nesting/result`
- `GET /support-strategies/catalog`
- `GET /fabrication/support-strategies/catalog`
- `POST /support-strategies/result`
- `POST /fabrication/support-strategies/result`
- `GET /process-recipes/catalog`
- `GET /fabrication/process-recipes/catalog`
- `POST /process-recipes/result`
- `POST /fabrication/process-recipes/result`
- `GET /kinematics/catalog`
- `GET /fabrication/kinematics/catalog`
- `POST /kinematics/result`
- `POST /fabrication/kinematics/result`
- `GET /tolerances/catalog`
- `GET /fabrication/tolerances/catalog`
- `POST /tolerances/result`
- `POST /fabrication/tolerances/result`
- `GET /process-capabilities/catalog`
- `GET /fabrication/process-capabilities/catalog`
- `POST /process-capabilities/result`
- `POST /fabrication/process-capabilities/result`
- `GET /manufacturability/catalog`
- `GET /fabrication/manufacturability/catalog`
- `POST /manufacturability/result`
- `POST /fabrication/manufacturability/result`
- `GET /failure-modes/catalog`
- `GET /fabrication/failure-modes/catalog`
- `POST /failure-modes/result`
- `POST /fabrication/failure-modes/result`
- `GET /safety/catalog`
- `GET /fabrication/safety/catalog`
- `POST /safety/result`
- `POST /fabrication/safety/result`
- `GET /environment/catalog`
- `GET /fabrication/environment/catalog`
- `POST /environment/result`
- `POST /fabrication/environment/result`
- `GET /provenance/catalog`
- `GET /fabrication/provenance/catalog`
- `GET /as-built/catalog`
- `GET /fabrication/as-built/catalog`
- `POST /as-built/result`
- `POST /fabrication/as-built/result`
- `POST /provenance/result`
- `POST /fabrication/provenance/result`
- `POST /setup/plan`
- `POST /fabrication/setup/plan`
- `POST /setup/result`
- `POST /fabrication/setup/result`
- `GET /monitoring/catalog`
- `GET /fabrication/monitoring/catalog`
- `POST /monitoring/plan`
- `POST /fabrication/monitoring/plan`
- `POST /monitoring/result`
- `POST /fabrication/monitoring/result`
- `GET /postprocess/catalog`
- `GET /fabrication/postprocess/catalog`
- `POST /postprocess/plan`
- `POST /fabrication/postprocess/plan`
- `POST /postprocess/result`
- `POST /fabrication/postprocess/result`
- `GET /evidence/catalog`
- `GET /fabrication/evidence/catalog`
- `GET /artifacts/catalog`
- `GET /fabrication/artifacts/catalog`
- `GET /packages/catalog`
- `GET /fabrication/packages/catalog`
- `POST /packages/plan`
- `POST /fabrication/packages/plan`
- `GET /learning/capabilities`
- `GET /fabrication/learning/capabilities`
- `GET /learning/engines/catalog`
- `GET /fabrication/learning/engines/catalog`
- `GET /learning/preflight/catalog`
- `GET /fabrication/learning/preflight/catalog`
- `GET /learning/features/catalog`
- `GET /fabrication/learning/features/catalog`
- `GET /learning/rewards/catalog`
- `GET /fabrication/learning/rewards/catalog`
- `GET /learning/models/catalog`
- `GET /fabrication/learning/models/catalog`
- `GET /learning/replay/catalog`
- `GET /fabrication/learning/replay/catalog`
- `GET /learning/beliefs/catalog`
- `GET /fabrication/learning/beliefs/catalog`
- `GET /learning/optimizers/catalog`
- `GET /fabrication/learning/optimizers/catalog`
- `POST /learning/models/result`
- `POST /fabrication/learning/models/result`
- `POST /learning/optimizers/result`
- `POST /fabrication/learning/optimizers/result`
- `GET /intake/catalog`
- `GET /fabrication/intake/catalog`
- `GET /templates/catalog`
- `GET /fabrication/templates/catalog`
- `GET /schema`
- `GET /fabrication/schema`
- `GET /examples`
- `GET /fabrication/examples`
- `GET /docs/api`
- `GET /api/docs`
- `GET /api/docs.json`
- `GET /jobs/catalog`
- `GET /fabrication/jobs/catalog`
- `GET /jobs`
- `GET /fabrication/jobs`
- `GET /jobs/:job_id`
- `GET /fabrication/jobs/:job_id`
- `GET /jobs/:job_id/release-bundle`
- `GET /fabrication/jobs/:job_id/release-bundle`
- `GET /jobs/:job_id/artifacts/:artifact_id`
- `GET /fabrication/jobs/:job_id/artifacts/:artifact_id`
- `GET /learning/policy`
- `GET /fabrication/learning/policy`
- `GET /learning/corpus`
- `GET /fabrication/learning/corpus`
- `GET /workflow/catalog`
- `GET /fabrication/workflow/catalog`
- `POST /plan`
- `POST /fabrication/plan`
- `POST /workflow/plan`
- `POST /fabrication/workflow/plan`
- `POST /instructions/analyze`
- `POST /fabrication/instructions/analyze`
- `POST /instructions/validate`
- `POST /fabrication/instructions/validate`
- `POST /instructions/validation/result`
- `POST /fabrication/instructions/validation/result`
- `POST /instructions/improve`
- `POST /fabrication/instructions/improve`
- `POST /instructions/boundaries/review`
- `POST /fabrication/instructions/boundaries/review`
- `POST /learning/observe`
- `POST /fabrication/learning/observe`
- `GET /learning/outcomes`
- `GET /fabrication/learning/outcomes`
- `POST /learning/outcomes`
- `POST /fabrication/learning/outcomes`

When `NATS_URL` is configured, the service also queue-subscribes to
`dd.remote.fabrication.requests` with queue group `dd-fabrication-server`,
publishes responses to `dd.remote.fabrication.results`, emits compact lifecycle
events to `dd.remote.events`, and can publish optimizer-shaped learning jobs to
`dd.remote.mdp.optimize` when `FABRICATION_MDP_AUTOPUBLISH=true`. Generated
machine code is intentionally advisory: responses are draft planning artifacts
and are not marked machine-ready.
The Rust deployment imports the local `des_engine` crate from
`remote/submodules/discrete-event-system.rs` as the preferred in-process
math/simulation/learning engine. Learning responses and optimizer artifacts
identify the DES SDK surface, carry canonical DES MDP/POMDP schema names, and
include DES-compatible `desMdpSpec`/`desPomdpSpec` payloads plus
value-iteration `desMdpSolution` and QMDP-underlying `desPomdpSolution`
previews for downstream policy workers. Plan responses also expose a DES Studio
`desScheduleModel` queue graph and `instructionIntentMap` so schedulers and
learning workers can analyze machine-lane capacity and generated/submitted
instruction intent from the same request. Instruction-analysis responses expose a
matching DES Studio `desInstructionModel` queue graph and the same
`instructionIntentMap` contract so imported CNC, slicer, printer, probing, joining, and text instruction streams can be prioritized by review capacity, failure-boundary pressure,
normalized process intent, and release handoff lane.
Its `reviewPriorities` rows give deterministic queue ordering for
machine-failure boundaries, human-intervention checkpoints, split/combine or
interface review, non-G-code job-sheet evidence extraction, and learning
feedback after disposition.
`GET /learning/capabilities`, `GET /fabrication/learning/capabilities`,
`GET /learning/engines/catalog`, and
`GET /fabrication/learning/engines/catalog` expose that local DES-backed
learning surface as `dd.fabrication.learning-capability-catalog.v1`, including
`des_engine::des::decision::solve_mdp`, `solve_pomdp_underlying`, DES Studio
queue graph analysis, and `FeedForwardNetwork` neural-policy previews.

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
  Parasolid/ACIS kernel files, DXF/DWG sheet-profile drawings,
  PrusaSlicer/OrcaSlicer/Cura/Bambu Studio FDM project sources, and
  Lychee Slicer/Chitubox resin project sources while retaining
  translator, topology, scale, PMI/tessellation, kernel-version/body-count,
  color/material/texture, layer/kerf/revision, slicer-profile,
  resin-exposure/support/wash-cure evidence, and release blockers.
  Its `conversionPlan` lists per-input CAD/model/slicer conversion worker lanes,
  design-conversion NATS request/result subjects, preferred neutral exports,
  required evidence, review gates, and machine-release blockers.
- A process plan and structured `processGraph` across 3D printers,
  robotic/gantry additive cells, sheet-lamination/LOM/UAM printers,
  vertical/5-axis/4th-axis/horizontal mills, routers, laser, waterjet, plasma, wire EDM/sheet cutters,
  sinker/ram EDM cells, precision grinding/lapping/honing cells, CMM/vision inspection cells, thermal postprocess furnace/oven cells, robotic assembly/joining cells,
  mill-turn/swiss-turning centers, and lathes when those machine profiles are
  available. The graph links operations,
  generated programs, sequencing dependencies, assembly interfaces, and release
  gates.
- An `instructionPrograms` stream retained as the `instruction-programs` artifact
  that normalizes generated drafts and submitted existing instructions together
  before validation, simulation, intent mapping, improvement, and MDP/POMDP
  learning.
- A `machineSelection` trace for every inferred or requested part, including the
  selected machine, required process class, candidate scores, material/process
  rejection reasons, operation gaps, and fallback warnings.
- A `productionPlan` with quantity-aware batches, setup-repeat counts,
  estimated machine minutes, review gates, release blockers, and unattended-run
  eligibility for each part/machine route.
- A `machineSchedule` with deterministic machine lanes, operation start/end
  windows, setup/run/teardown minutes, process dependency holds, postprocessor
  holds, and operator or automation assignments before machine start.
- A DES Studio `desScheduleModel` with per-machine `Constant -> Queue -> Sink`
  blocks, service-rate estimates, structural analysis, and lane mappings for
  resource-capacity simulation.
- Draft machine programs such as Marlin-style FDM printer G-code and slicer job
  sheets, multi-material FDM/toolchanger job sheets with material/color map,
  AMS/MMU/IDEX/toolhead slots, purge/wipe tower, tool-change script, runout/resume-state,
  and interface inspection gates, large-format pellet/FGF job sheets with pellet lot, drying/moisture,
  hopper/purge, bead/thermal/cooling, gantry-clearance, warpage, and trim-allowance
  gates, robotic/gantry additive job sheets with robot frame/TCP, reach,
  collision simulation, external-axis/positioner sync, interlock dry run,
  feedstock/nozzle purge, bead coupon, flow pressure, cooling/cure,
  trim-allowance, and dimensional-scan
  gates, sheet-lamination/LOM/UAM job sheets with sheet/foil lot,
  thickness/gauge, stack order, registration/fiducials, adhesive or ultrasonic
  consolidation, trim/cut path, coupon/peel or lap-shear, delamination, and
  dimensional-release gates, paste/clay extrusion job sheets with rheology/slump,
  nozzle/pressure, drying/humidity, shrinkage, green-part support, and kiln/firing gates,
  bound-metal filament FFF job sheets with filament/profile, hardened-nozzle,
  green-part, debind, sinter, furnace-atmosphere, shrinkage-coupon, density, and
  inspection gates,
  SLA/MSLA resin print-wash-cure job sheets, PolyJet/material-jetting
  photopolymer job sheets with cartridge, channel-map, printhead, support-removal,
  UV, and color/material inspection gates, continuous-fiber composite
  matrix/fiber-layup job sheets with fiber orientation, cutter, spool, coupon, and
  delamination gates, composite layup/vacuum-bag/autoclave job sheets with
  mold/mandrel revision, release film or agent, ply kit/schedule,
  resin/prepreg/core lots, vacuum-bag leak-down, debulk/cure trace, demold,
  trim/drill, coupon, NDI/void/delamination, and dimensional-release gates,
  hot-wire foam cutting job sheets with foam density, blank thickness, template
  or CNC profile, bow/wire tension, wire heat/current, kerf coupon, fume
  extraction/PPE/fire-watch, taper/surface-melt, and dimensional-release gates,
  SLS/MJF-style powder-bed
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
  gates, precision grinding job sheets with wheel dress/balance, coolant,
  magnetic chuck or centers, spark-out, burn/chatter, surface-finish, and
  final-metrology gates, CMM/vision inspection job sheets with probe or vision
  calibration, datum alignment, first-article measured values, tolerance
  disposition, and nonconformance-routing gates, thermal postprocess furnace job
  sheets with material batch, fixture/setter, ramp/soak, atmosphere,
  cooldown/quench, PPE, distortion, hardness/cure, and release-inspection gates,
  surface finishing/coating/plating/anodizing/media-blasting/powder-coating/deburr-polish job sheets with material compatibility,
  SDS/media, masking/plugs, ventilation/PPE/waste, dry/cure, thickness,
  adhesion/color/roughness, dimensional-impact, and finish-inspection gates,
  metal-joining/welding/brazing/soldering job sheets with WPS/procedure,
  qualification, filler/flux/gas lots, joint prep, fit-up, fixture/clamps,
  fume controls, heat input, interpass temperature, distortion control,
  inspection/NDE/leak-test, repair-disposition, and release gates,
  molding/casting/vacuum-casting/urethane/silicone/injection-molding job
  sheets with master/tool revision, mold material, parting line,
  vents/sprues/runners/gates, release agent, mix ratio, pot life,
  degas/vacuum/pressure, cure/exotherm, demold, shrinkage, void/bubble/flash,
  dimensional-inspection, and release gates,
  press-brake/sheet-forming job sheets with flat-pattern revision, grain
  direction, material thickness, bend allowance/K-factor, punch/V-die tooling,
  tonnage, backgauge, bend sequence, springback, guarding, angle/flange
  inspection, and formed-part release gates,
  gear-cutting/hobbing/spline-broaching job sheets with gear drawing, blank
  datum, arbor/runout, cutter or hob/shaper/broach qualification, tooth count,
  module or diametral pitch, pressure/helix/lead angle, index ratio/change-gear
  or electronic-gearbox synchronization, deburr/burr-control, over-pins/span,
  profile/runout, backlash, and gear-inspection gates,
  robotic assembly-cell job sheets with kit traceability, datum dry-fit,
  robot path/gripper/fixture/vision evidence, press/heat-set/torque/adhesive
  join recipes, cure or clamp timing, and final metrology gates, mill-turn
  G-code with C/Y-axis live-tooling and subspindle transfer checkpoints,
  Swiss/sliding-headstock G-code with guide-bushing, bar-feed, gang-tool/live-tool,
  subspindle pickoff, cutoff/ejection, and first-article runout checkpoints,
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
  high-speed kinematic evidence, multi-material FDM material/color map, slot, filament-lot,
  support-interface, purge/wipe tower, tool-change script, runout-sensor, and resume-state evidence,
  additive thin-wall geometry, printer
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
  resin layer/exposure manifest image-hash/checksum and peel/lift/recoat evidence,
  including generated `EXPOSE`/`PEEL` image-stack records and peel/lift/recoat evidence, resin
  vat-capacity/refill evidence, resin-handling/postprocess evidence,
  pellet/FGF pellet-lot/drying/moisture/hopper/purge evidence from generated `DRY_PELLETS`/`PURGE_EXTRUDER` records and bead/screw/melt/cooling/gantry-clearance/warpage/trim-allowance evidence from generated `PRINT_BEAD_PATH`/`MONITOR` records,
  robotic additive robot frame/TCP/reach/collision/interlock/external-axis evidence from generated `LOAD_ROBOT_PATH`/`DRY_RUN_ROBOT` records and feedstock/nozzle/purge/bead/flow/cooling/cure/dimensional-scan evidence from generated `PURGE_ROBOTIC_EXTRUDER`/`DEPOSIT_ROBOTIC_BEAD_PATH`/`MONITOR` records,
  sheet-lamination sheet/foil stock/stack-order/surface-prep evidence from generated `LOAD_SHEET_STACK` records and registration/trim/bond/consolidation/delamination/dimensional-release evidence from generated `REGISTER_LAYER_STACK`/`CUT_OR_TRIM_LAYERS`/`BOND_OR_CONSOLIDATE_LAYERS`/`INSPECT_LAMINATION` records,
  paste/clay rheology/slump/deairing/nozzle/pressure evidence from generated `CONDITION_PASTE`/`PURGE_SYRINGE_OR_AUGER` records and drying/humidity/shrinkage/green-part/firing evidence from generated `PRINT_PASTE_PATH`/`DRY_GREEN_PART` records,
  bound-metal filament profile/nozzle/dry-storage/shrinkage evidence from generated `LOAD_BOUND_METAL_FILAMENT`/`SLICE_BOUND_METAL_FFF`/`PRINT_GREEN_PART` records and debind/sinter/furnace/atmosphere/density inspection evidence from generated `DEBIND_GREEN_PART`/`SINTER_PART` records,
  material-jetting cartridge/channel-map/printhead/tray plus generated `PACK_TRAY`/`JET_MATERIALS` evidence and support-removal/UV/color/material inspection evidence from generated `REMOVE_SUPPORT`/`UV_CURE_INLINE` records,
  DED/WAAM feedstock/substrate/bead-path/standoff plus generated `PREP_SUBSTRATE`/`PLAN_BEADS` evidence and laser/arc/shielding/interpass/NDE/coupon evidence from generated `START_DEPOSITION`/`MONITOR_MELT_POOL`/`INSPECT_DEPOSIT` records,
  composite-fiber layup/orientation/load-case evidence from generated `FIBER_LAYUP` records and spool/cutter/matrix/coupon/continuity evidence from generated `FIBER_CUT_ANCHOR`/`PRINT_COMPOSITE`/inspection records,
  composite layup mold/mandrel/release-system/ply-schedule/resin-prepreg-core-lot evidence from generated `PREPARE_LAYUP_TOOL`/`LAYUP_PLIES` records and vacuum-bag/leak-down/cure/demold-trim-inspection evidence from generated `VACUUM_BAG_AND_LEAK_TEST`/`CURE_LAMINATE`/`DEMOLD_TRIM_INSPECT` records,
  hot-wire foam blank/density/thickness/support evidence from generated `FOAM_BLANK_SETUP` records and wire-heat/tension/kerf/feed/taper/cut/release evidence from generated `WIRE_HEAT_TENSION_CHECK`/`KERF_COUPON`/`HOT_WIRE_CUT` records,
  press-brake flat-blank/tooling/bend-sequence evidence from generated `LOAD_FLAT_BLANK`/`SET_BRAKE_TOOLING`/`RUN_BEND_SEQUENCE` records and formed-part angle/flange/radius/flatness release evidence from generated `INSPECT_FORMED_PART` records,
  gear/spline blank-datum/tool/module-or-DP/index-ratio/tooth/deburr/inspection evidence from generated `LOAD_GEAR_BLANK`/`SET_GEAR_TOOL`/`CUT_GEAR_TEETH`/`DEBURR_PROFILE`/`INSPECT_GEAR` records,
  powder-bed build profile/powder lot/nesting evidence from generated `NEST`/`PRINT` records, powder-handling/cooldown-depowder evidence from generated `DEPOWDER`/cooldown records,
  metal powder-bed fusion alloy-lot/oxygen/recoater/stress-relief/plate-removal evidence from generated `BUILD_ORIENT`/`INERT_GAS_PURGE`/`RECOATER_CLEARANCE_CHECK`/`PRINT_METAL_PBF`/`STRESS_RELIEF`/`PLATE_REMOVAL` records,
  binder-jet binder-lot/saturation/printhead/green-strength plus generated `BINDER_JET_PRINT` evidence and cure/debind/sinter/infiltration/shrink-compensation evidence from generated `CURE_GREEN_PART`/`SINTER_OR_INFILTRATE` records,
  powder-bed recoater clearance/thermal spacing/cooldown evidence,
  assembly fit/metrology/datum/torque/cure evidence, assembly-cell kit/revision/join-graph and dry-fit/datum evidence from generated `KIT_PARTS`/`VERIFY_DATUMS` records, robot-path/gripper/collision/vision evidence from generated `PICK_PLACE` records, and press-fit/heat-set/torque/adhesive-cure plus vision/pull-or-torque/go-no-go/final-metrology release evidence from generated `JOIN`/`INSPECT_JOIN` records,
  part-separation fixture/hold-down/cut-path/kerf evidence from structured `LOAD_SEPARATION_FIXTURE`/`CUT_PATH` records and tab-release/deburr/traceability/final-inspection evidence from structured `RELEASE_RETAINED_TABS`/`DEBURR_EDGES`/`TRACE_PARTS`/`INSPECT_SEPARATION` records,
  precision tolerance/surface-finish metrology evidence, precision-grinding wheel dress/workholding evidence from generated `DRESS_WHEEL`/`SETUP_WORKHOLDING` records and spark-out/final-metrology release evidence from generated `SPARK_OUT`/`INSPECT_GRIND` records, and CMM/vision calibration/datum/feature evidence from generated `CALIBRATE_PROBE`/`ALIGN_DATUMS`/`MEASURE_FEATURE` records plus measured-values/pass-fail/nonconformance release evidence from generated `REPORT_INSPECTION` records,
  unattended/batch monitoring evidence plus separate restart/recovery/operator-check-in/batch-inspection evidence,
  thermal postprocess batch/fixture/setter/spacing evidence from generated `LOAD_THERMAL_BATCH` records, profile/ramp/soak/atmosphere evidence from generated `RUN_THERMAL_PROFILE` records, cooldown/quench/safe-handling evidence from generated `CONTROL_COOLDOWN` records, and distortion/shrinkage/hardness-or-cure/pass-fail release evidence from generated `INSPECT_THERMAL_RELEASE` records,
  surface/chemical finishing protected-surface/thread/datum/cosmetic-face evidence from generated `MASK_FEATURES` records, process/media-or-chemistry/dwell/agitation-or-blast-pressure evidence from generated `RUN_SURFACE_FINISH` records, and thickness/roughness-or-color/adhesion/dimension/pass-fail release evidence from generated `INSPECT_SURFACE_FINISH` records,
  metal-joining joint-design/edge-prep/fit-up/fixture evidence from generated `PREP_JOINTS` records, process/WPS/filler-or-solder/shielding-or-flux evidence from generated `SET_JOINING_PROCESS` records, heat-input/travel-speed/interpass/tack-sequence/distortion-control evidence from generated `RUN_METAL_JOIN` records, and visual/fillet-or-penetration/distortion/NDE-or-leak-test/pass-fail release evidence from generated `INSPECT_JOIN` records,
  molding/casting master/tool-revision/release-agent/vent/parting-line evidence from generated `PREPARE_MOLD` records, material/mix-ratio/pot-life/batch evidence from generated `MIX_CASTING_MATERIAL` records, vacuum/pressure/fill-strategy/temperature evidence from generated `DEGAS_AND_CAST` records, and demold/flash/void/shrinkage/dimensional-release evidence from generated `DEMOLD_AND_INSPECT` records,
  press-brake/sheet-forming flat-pattern/bend-allowance/tooling/tonnage/backgauge/springback/angle-inspection evidence,
  gear-cutting gear-drawing/tooth-count/module-or-DP/pressure-angle/helix-lead/cutter-arbor/index-ratio/blank-runout/deburr/over-pins/span/profile/backlash inspection evidence,
  indexed setup clamp/index/clearance/re-probe evidence,
  sheet-cutting material/thickness/cut-chart/recipe evidence, generated sheet-cutting setup/cut-path/release evidence, pierce/kerf/focus/gas/fume/support, retained-tab/microjoint/part-release evidence, waterjet pressure/abrasive-flow, plasma work-clamp evidence, wire EDM start-hole/thread/tension/dielectric/flushing/slug-retention/skim-pass evidence plus profile/skim-cut setup-order evidence, and sinker EDM electrode/dielectric/depth/wear/orbit-finish/recast release-gate evidence from generated `KERF_TEST`/`PIERCE`/`VECTOR_CUT`/`WATERJET_CUT`/`PLASMA_CUT` records and generated `ELECTRODE_VERIFY`/`DIELECTRIC_FLUSH_TEST`/`ROUGH_BURN`/`DEPTH_CHECK`/`ORBIT_FINISH` records,
  sinker EDM electrode/dielectric/depth/wear/orbit-finish/recast release-gate evidence from generated `ELECTRODE_VERIFY`/`DIELECTRIC_FLUSH_TEST`/`ROUGH_BURN`/`DEPTH_CHECK`/`ORBIT_FINISH` records,
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
  router, sheet cutter, lathe, inspection cell, robotic additive cell, sheet-lamination printer, hot-wire foam cutter,
  gear-cutting cell, robotic assembly cell, or manual cell can start.
- A `controllerPlan` compatibility contract with controller dialect families,
  postprocessor-known status, required controller checks, required evidence,
  controller release gates, blockers, and `controller-*` learning observations
  before controller output can be treated as machine-ready.
- A `manufacturingHandoff` package with part-level geometry envelopes, stock
  strategy, datum scheme, fixture/setup plan, inspection gates, release blockers,
  and release gates for downstream CAD/CAM, slicer, or shop-floor review.
- A `materialPlan` with route feedstock, stock forms, quantity estimates, scrap
  allowances, conditioning steps, required material/stock evidence, release gates,
  blockers, and compact `material-route:*` learning observations.
- A `qualityPlan` with inspection points, measurement targets, records to
  capture, release gates, and learning observations for MDP/POMDP/neural outcome
  feedback after shop-floor evidence is recorded.
- A `toolingPlan` setup traveler with required tools, workholding, consumables,
  setup checks, automation dependencies, release blockers, and production-batch
  links for each generated part route.
- A `fixturePlan` with per-part setup strategies, datum schemes, workholding,
  required fixture evidence, clearance checks, datum-transfer records, release
  blockers, automation candidates, and `fixture-*` learning observations for
  MDP/POMDP workers.
- A `monitoringPlan` with runtime sensor channels, expected signals, alert rules,
  recovery actions, release blockers, and `monitoring-*` learning observations so
  jobs can feed live machine evidence into MDP/POMDP/neural outcome loops.
- An `interfaceControlPlan` for join/split interfaces, mating-surface evidence,
  acceptance criteria, split/combine decision links, release blockers, and
  `interface-*` learning observations before combining, separating, or
  machine-ready release.
- A `decompositionPlan` with explicit split targets, route contracts,
  recomposition interfaces, release gates, blockers, and `decomposition-*`
  learning observations so workers can prove when one body must become multiple
  parts, or multiple parts can be recombined safely.
- A `releasePackagePlan` that bundles each generated machine program, imported
  `instructionPrograms` stream, or assembly/recomposition handoff with retained
  `instruction-programs` analysis, design export IDs, controller targets,
  fixture setups, monitoring points, quality inspections, decomposition targets,
  interface controls, required artifacts, release gates, blockers, and
  `release-package*` learning observations for downstream worker review.
- `improvements` and `improvedPrograms` review drafts for generated and
  submitted instruction streams, with conservative gates inserted before
  machine-ready release and a `patchManifest` that records line-level repair
  operations, review reasons, inserted content, and learning observations.
- Assembly advice with a structured `assemblyGraph` of part nodes, hybrid
  interface edges, join/fit strategies, inspection gates, and sequence steps for
  deciding when parts should be combined into one job or split so tight-tolerance
  features can be machined and inspected separately.
- A `hybridMakePlan` with part routes, join operations, split/combine decisions,
  review-gated actions, and compact learning observations so MDP/POMDP/neural
  workers can compare future single-piece, split-piece, and assembled outcomes.
- A learning contract with MDP states, POMDP observations, a structured
  `pomdpBeliefState` with hidden-state probabilities and probe actions, a
  `releaseProbePlan` of priority evidence probes required before release, policy
  actions, scored `strategyCandidates`, typed `interventionSignals`, reward terms,
  neural feature names, a DES `FeedForwardNetwork`-backed neural-policy sketch
  with `neuralPolicy.engineInference`, and
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
- `GET /fabrication/strategy/catalog` exposes the advisory hybrid route,
  learned preference, MDP/POMDP policy, intervention, and neural-corpus
  strategy contracts that feed those choices without clearing release gates.
- `GET /fabrication/methods/catalog` exposes additive, milling/routing,
  turning, sheet-cutting, hybrid split/combine, postprocess, inspection, and
  special-process method families so clients can discover which process
  families the planner may print, mill, turn, cut, inspect, join, or learn from.
- A bounded in-process job and artifact ledger for generated design summaries,
  parametric design payloads, design packages, design export bundles, process plans,
  production plans, machine programs, validation reports, boundary summaries,
  resolution plans, intervention maps, execution plans, postprocess plans, POMDP
  belief states, release probe plans, neural training corpora, machine-release reports, manufacturing
  handoffs, material plans, quality plans, tooling plans, interface-control plans, machine-selection traces, improved instructions,
  assembly plans, process graphs, assembly graphs, and optimizer-shaped MDP
  requests.

Real production use still requires CAD/CAM generation, controller-specific
post-processing, simulation, workholding review, material verification, and
operator sign-off.

## `GET /fabrication/capabilities`

`GET /capabilities` and the gateway-prefixed `GET /fabrication/capabilities`
return the service capability contract before a caller submits work. The payload
includes supported request families, built-in `defaultMachines`, machine classes
for FDM, multi-material FDM/toolchanger, large-format pellet/FGF, robotic/gantry additive cells, sheet-lamination/LOM/UAM printers, paste/clay extrusion, bound-metal filament FFF, resin, material jetting, directed-energy deposition/WAAM, continuous-fiber composite, composite layup/vacuum-bag/autoclave cells, hot-wire foam cutters, binder jet, polymer powder-bed, metal PBF, vertical milling, five-axis milling, rotary-indexed milling, horizontal milling, precision grinding/lapping/honing, CMM/vision inspection, thermal postprocess furnace/oven work, surface/chemical finishing and coating cells, metal-joining/welding/brazing/soldering cells, molding/casting/vacuum-casting/urethane/silicone/injection-molding cells, PCB/SMT electronics assembly cells, press-brake/sheet-forming cells, gear-cutting/hobbing/spline-broaching cells,
mill-turn/swiss-turning, routing, laser,
waterjet, plasma, robotic assembly/joining, lathe, and manual/special-process
work, accepted instruction kinds including slicer, multi-material FDM/toolchanger, pellet-FGF, robotic-additive, robotic-pellet, robotic-extrusion, sheet-lamination, laminated-object, ultrasonic-additive, paste/clay extrusion, bound-metal FFF,
metal-filament, SLA/resin,
material-jetting, DED/WAAM, composite-fiber, composite-layup, wet-layup, prepreg-layup, vacuum-bag, autoclave-cure, resin-infusion, hot-wire-foam, hot-wire, foam-cutting, foam-core, wing-core, binder-jet, SLS/powder, metal-PBF,
mill-turn, swiss-turning, lathe/turning, indexed-mill, assembly-cell, part-separation, laser/waterjet/plasma,
wire-EDM, sinker-EDM, grinding, CMM inspection, vision inspection, metrology, furnace, heat-treatment, thermal-postprocess, surface-finishing, coating, plating, anodizing, media-blasting, powder-coating, deburr-polish, metal-joining, welding, brazing, soldering, molding-casting, casting, molding, urethane-casting, silicone-molding, vacuum-casting, injection-molding, PCB assembly, SMT assembly, pick-and-place, reflow, press-brake, sheet-forming, bend, gear-cutting, gear-hobbing, and spline-broaching job sheets, design input format
families, generated artifact families including `learning-policy-snapshot`,
`learning-outcome-memory`, and `learning-corpus`, learning channels, a
`machineFleetLimits` block for the bounded fallback/submitted machine fleet of up to 96 profiles,
bounded `profileEvidence` buckets for submitted machine profiles, and safety boundary
classes. Discovery routes include
`GET /machines/catalog`,
`GET /fabrication/machines/catalog`, `POST /fabrication/machines/select`,
`GET /printers/catalog`,
`GET /fabrication/printers/catalog`, `GET /subtractive/catalog`,
`GET /fabrication/subtractive/catalog`, `GET /cnc/catalog`,
`GET /fabrication/cnc/catalog`, `GET /cells/catalog`,
`GET /fabrication/cells/catalog`, `GET /hybrid/catalog`,
`GET /fabrication/hybrid/catalog`, `GET /methods/catalog`,
`GET /fabrication/methods/catalog`, `POST /fabrication/controllers/result`,
plus `GET /fabrication/machine-code/catalog`,
`GET /fabrication/learning/engines/catalog`,
`GET /fabrication/learning/rewards/catalog`, and
`GET /fabrication/learning/corpus` so clients can inspect default machine,
bounded machine-selection,
additive-printer, mesh/topology repair review, CNC/subtractive machining/cutting,
hybrid split/combine, workflow route/evidence planning, manufacturing-method,
controller/postprocessor review, draft machine-code generation, costing, utilities, energy,
availability, maintenance, telemetry, consumables, workholding, process-capability,
safety/environment, provenance, DES engine, reward shaping, and neural training-corpus
capabilities before planning. The capability contract also advertises
`strategyQualitySurfaces` such as `policySummary.successRate`,
`policySummary.failureRate`, `policySummary.learnedQuality`,
`learningOutcomeQuality.riskReviewRequired`, and
`learningOutcomeQuality.releasePolicy` so clients can discover the learned
strategy review gate before calling `POST /fabrication/strategy/recommend`.
It also links `GET /fabrication/subjects/catalog`,
`GET /fabrication/workers/catalog`, and `GET /fabrication/results/catalog` so
external CAD/CAM, slicer, simulation, release, and learning workers can discover
dispatch subjects and result-review intake routes from the top-level capability
document.
These capabilities describe draft planning and validation support, not
controller-certified release.

## `GET /fabrication/machines/catalog`

`GET /machines/catalog` and the gateway-prefixed
`GET /fabrication/machines/catalog` return the live
`dd.fabrication.machine-catalog.v1` catalog derived from `default_machines()`.
The payload exposes the supported default fleet for additive printers,
large-format pellet/FGF, robotic/gantry additive cells, sheet-lamination/LOM/UAM printers, paste/clay extrusion, bound-metal filament FFF, resin, material jetting, fiber composite, composite layup/vacuum-bag/autoclave, hot-wire foam cutting, binder jet,
SLS/MJF/powder-bed, metal PBF, DED, vertical/horizontal/five-axis/indexed mills,
CNC routers, mill-turn centers, Swiss/sliding-headstock turning centers, lathes, laser/waterjet/plasma/wire EDM/sinker
EDM cells, precision grinders, CMM/vision inspection cells, thermal postprocess furnaces, surface finishing/coating cells, metal-joining/welding/brazing/soldering cells, molding/casting cells, PCB/SMT electronics assembly cells, composite layup cells, hot-wire foam cutters, press-brake/sheet-forming cells, gear-cutting/hobbing/spline-broaching cells, robotic additive cells, and robotic assembly cells. It includes machine kinds, process-class
counts, controllers, supported materials, operation tags, work envelopes, axes,
accepted instruction languages, planning and instruction-analysis route aliases,
and per-machine release gates. Catalog machines are default planning profiles,
not certified shop-floor assets; callers should attach bounded
`profileEvidence` to harden or override them before planning, and machine-ready
release remains blocked until profile evidence, controller/postprocessor checks,
simulation or dry-run review, and operator or automation signoff pass.
The catalog also exposes a `selectionEvidenceMatrix` for machine-family
selection. It spells out the evidence required before choosing FDM or
multi-material/pellet printers, resin and powder-bed printers, vertical,
horizontal, five-axis, or indexed mills, routers and sheet cutters, and lathes,
mill-turn, or Swiss machines. Selection rows connect machine choices to
`machineSelection.candidates`, material, fixture, controller, postprocess,
simulation, and release-package surfaces, and keep routing blocked when build
envelopes, horizontal/rotary access, hold-down or slug support, thermal
readiness, threading/feed synchronization, or controller dry-run evidence is
missing.
Resin-printer entries advertise `ctb-resin-job`, `photon-resin-job`,
`lychee-resin-job`, and `chitubox-resin-job` alongside `sla-job` and `resin-job`
so machine discovery aligns Lychee/Chitubox/Photon/CTB slice packages with the
resin exposure, peel/lift/recoat, wash/cure, resin lot, and PPE gates.

## `GET /fabrication/printers/catalog`

`GET /printers/catalog` and the gateway-prefixed
`GET /fabrication/printers/catalog` return the live
`dd.fabrication.printer-catalog.v1` discovery view for additive printer and
printer-like cell profiles derived from `default_machines()`. The catalog filters
the machine catalog to FDM, multi-material FDM/toolchanger, pellet/FGF,
paste/clay extrusion, sheet-lamination, bound-metal FFF, SLA/resin, material
jetting, continuous-fiber composite, SLS/powder-bed, binder jet, metal PBF, and
DED/WAAM directed-energy deposition profiles. Each printer entry retains
materials, work envelope, axes, accepted instruction languages, operations,
release gates, and required evidence for printer capability, material/feedstock
conditioning, slicer or generated job profile, build surface or powder/vat
readiness, simulation/first-article review, telemetry, and learning outcome
retention. Printer catalog entries are default additive planning profiles, not
certified live printer availability; machine-ready release remains blocked until
the printer profile, material, slicer/job-sheet, support/build-surface,
simulation, and operator or automation evidence clear.

## `GET /fabrication/subtractive/catalog`

`GET /subtractive/catalog` and the gateway-prefixed
`GET /fabrication/subtractive/catalog` return the live
`dd.fabrication.subtractive-catalog.v1` discovery view for machining and cutting
profiles derived from `default_machines()`. The catalog filters the machine
catalog to vertical, horizontal, five-axis, and indexed mills, CNC routers,
lathes, mill-turn and Swiss turning centers, laser/waterjet/plasma/wire EDM
sheet cutting, plus sinker EDM, precision grinding, and gear cutting profiles.
Each entry retains materials, work envelope, axes, accepted instruction
languages, operations, release gates, and required evidence for machine
capability, stock, workholding, datum/work-offset, tooling, feed/speed, coolant,
chip evacuation, beam/jet/abrasive/gas/support-media readiness, dry-run or
simulation, first-article review, telemetry, and learning outcome retention.
Subtractive catalog entries are default planning profiles, not certified live
machine availability; machine-ready release remains blocked until the setup,
controller/postprocessor, process media, simulation, and operator or automation
evidence clear.

## `GET /fabrication/subtractive/preflight/catalog`

`GET /subtractive/preflight/catalog` and the gateway-prefixed
`GET /fabrication/subtractive/preflight/catalog` return the live
`dd.fabrication.subtractive-preflight-catalog.v1` machining and cutting release
checklist. The catalog groups stock/workholding/datum state,
tool/process/media state, and controller/geometry/simulation state before
generated or imported mill, router, lathe, sheet-cutting, EDM, grinding, or gear
cutting instructions can move toward release. It calls out stock orientation,
fixtures, vises, chucks, collets, tailstock/subspindle/catcher support,
work-offset/probe/setup-sheet proof, tool and offset evidence, positive spindle,
beam, pump, abrasive, gas, coolant, chip-evacuation, dust, fume, or support-media
readiness, feed/speed evidence, modal controller review, exact-program dry-run,
first-article, metrology, telemetry, and release-package checksum gates.

Subtractive preflight entries are release evidence contracts, not live machine
approvals. Failed checks should be retained through setup, simulation, quality,
telemetry, and learning outcome routes so DES, MDP/POMDP, and neural workers can
learn safer split/combine and machine-routing strategies.

## `GET /fabrication/turning/preflight/catalog`

`GET /turning/preflight/catalog` and the gateway-prefixed
`GET /fabrication/turning/preflight/catalog` return the live
`dd.fabrication.turning-preflight-catalog.v1` release checklist for lathe,
Swiss/sliding-headstock, bar-fed, and mill-turn instructions. The catalog groups
chuck/collet/bar-stock/support state, turning tooling/offset/threading state, and
mill-turn live-tool/spindle-transfer state before generated or imported turning
programs can move toward machine-ready review.

Turning preflight keeps lathe, Swiss, and mill-turn programs blocked until
workholding, stick-out/runout, subspindle or tailstock support, part-off capture,
tool-nose and wear offsets, feed-per-rev or threading synchronization, C/Y/B-axis
live tooling, spindle-transfer, simulation/dry-run, quality, and signoff evidence
clear. Failed checks feed DES, MDP/POMDP, reward, neural, and learning-outcome
workers so future plans can split or combine turned inserts, reroute machines, or
insert human checkpoints before risky turning boundaries.

## `GET /fabrication/cleanliness/preflight/catalog`

`GET /cleanliness/preflight/catalog` and the gateway-prefixed
`GET /fabrication/cleanliness/preflight/catalog` return the live
`dd.fabrication.cleanliness-preflight-catalog.v1` release checklist for residue,
FOD, drying, and interface cleanliness before printed and machined parts are
joined, inspected, coated, packed, or handed to an operator. The catalog groups
additive residue and powder state, machining coolant/chip/FOD state, and
assembly-interface cleanliness. It calls out resin drip/wash/cure evidence,
powder depowdering and trapped-powder removal, soluble support and moisture
state, coolant/oil/abrasive/dielectric/chip removal, blind-hole or internal
channel cleaning, chemistry compatibility, critical surface cleanliness, swab or
particle evidence, packout cleanliness class, caps/plugs, desiccant, traveler,
and release-owner proof.

Cleanliness preflight entries are release evidence contracts, not a released
cleaning specification. Failed checks should be retained through postprocess,
quality, release bundles, and learning outcome routes so DES, MDP/POMDP, and
neural workers can learn when to split, combine, clean, reroute, or require human
intervention before contaminated parts are joined.

## `GET /fabrication/interfaces/preflight/catalog`

`GET /interfaces/preflight/catalog` and the gateway-prefixed
`GET /fabrication/interfaces/preflight/catalog` return the live
`dd.fabrication.interface-preflight-catalog.v1` split/combine release checklist
for recombining printed, milled, turned, molded, or processed parts. The catalog
groups datum and locating-interface state, fit/tolerance/stackup state, and
joining hardware/service-interface state. It calls out datum transfer,
orientation/keying, shrink/warp compensation, machined datum cleanup,
critical-to-function dimensions, clearance/interference fits, thread/bore/shaft
gauges, postprocess thickness allowances, bondline/weld/seal compression,
go/no-go and functional proof, fasteners, inserts, seals, adhesives, welds,
service access, kit genealogy, interface revision, and release-owner evidence.

Interface preflight entries are recomposition evidence contracts, not released
drawings or assembly work instructions. Failed checks should be retained through
decomposition, assembly, quality, release, and learning outcome routes so DES,
MDP/POMDP, and neural workers can learn when to split differently, add datums,
change joining methods, route rework, or require human intervention before
parts are combined.

## `GET /fabrication/cells/catalog`

`GET /cells/catalog` and the gateway-prefixed
`GET /fabrication/cells/catalog` return the live
`dd.fabrication.cell-catalog.v1` discovery view for hybrid, robotic, inspection,
and process cells derived from `default_machines()`. The payload filters the
machine catalog into cell families such as robotic additive, directed-energy
deposition, robotic assembly, inspection/metrology, thermal postprocess,
surface finishing, metal joining, molding/casting, PCB/SMT electronics assembly, composite layup,
sheet-forming, gear-cutting, and sinker EDM. Each cell entry retains the
machine kind, process class, controller or job-sheet dialect, materials,
operation tags, work envelope, axes, accepted instruction languages, release
gates, and required evidence for capability profile, fixture/workholding or
end-effector proof, dry-run/simulation, handoff, telemetry, and learning
outcome retention. Cell catalog entries are default planning profiles, not
certified robot, furnace, joining, metrology, or process-cell approvals; machine-ready
release remains blocked until cell capability, interlock, controller, fixture,
dry-run, and operator or automation evidence are retained.

## `POST /fabrication/machines/select`

`POST /machines/select` and the gateway-prefixed
`POST /fabrication/machines/select` accept the same request body as
`POST /fabrication/plan`, apply bounded learning-policy memory, run the planner,
store and publish the full plan result, and return a compact
`dd.fabrication.machine-selection.v1` machine-routing package. The response
focuses on `machineSelection`, `machineSelection.selectedMachineId`,
`machineSelection.selectedMachineKind`, `machineSelection.selectedReason`,
`machineSelection.candidates`, `machineSelection.candidates.status`,
`machineSelection.candidates.reasons`, `machineSelection.warnings`,
`machineSchedule.machineLanes`, `machineSchedule.operations`,
`machineSchedule.dependencyHolds`, `materialPlan.routeRequirements`,
`controllerPlan.compatibilityTargets`, `postprocessPlan.controllerTargets`,
`machineRelease.blockers`, and `simulation.programs`.

Machine selection plans are advisory routing evidence for printers, mills,
lathes, routers, sheet cutters, inspection cells, and special-process machines,
not certified live shop availability. Machine-ready release remains blocked
while selected-machine profile evidence, material compatibility,
controller/postprocessor output, schedule holds, simulation, setup, quality, or
signoff gates are unresolved. Stored artifacts include `machine-selection`,
`machine-schedule`, `des-schedule-model`, `material-plan`, `controller-plan`,
`postprocess-plan`, `machine-release`, `simulation-report`, and `mdp-request` so
MDP/POMDP/neural workers can learn machine preferences, split/combine routes,
and fallback decisions from future outcomes.

## `GET /fabrication/controllers/catalog`

`GET /controllers/catalog` and the gateway-prefixed
`GET /fabrication/controllers/catalog` return the live
`dd.fabrication.controller-postprocessor-catalog.v1` controller and
postprocessor discovery catalog derived from the current `default_machines()`
fleet plus the same `postprocessor_for`, output-format, and controller dialect
logic used by `postprocessPlan` and `controllerPlan`. The payload exposes
machine/controller targets, process classes, controller dialect families,
postprocessor names, postprocessor-known counts, output formats, required
controller checks, release evidence requirements, planning and
instruction-analysis route aliases, and response surfaces such as
`postprocessPlan.controllerTargets`, `controllerPlan.compatibilityTargets`,
`controllerPlan.dialectSummaries`, `controllerPlan.releaseGates`, and
`releasePackagePlan.packages`. Machine-ready release remains blocked until the
exact postprocessed output, controller setup sheet, dry-run or simulation
evidence, postprocessor version, and operator or automation signoff are retained;
unknown controller/postprocessor combinations stay routed to manual controller
review.

The catalog also publishes a `dialectAssumptionChecklist` for modal defaults and
reset state, offset tables and compensation, macro/subprogram dependencies, and
postprocessed-output dry-run proof. Each item names the controller surfaces it
blocks, including `instructionValidation.boundaries`, `controllerPlan.releaseGates`,
and `machineRelease.blockers`, plus learning signals such as
`controller-modal-defaults:*`, `macro-dependency:*`, and `dry-run-proof:*`.

`GET /controllers/preflight/catalog` and the gateway-prefixed
`GET /fabrication/controllers/preflight/catalog` return the live
`dd.fabrication.controller-preflight-catalog.v1` checklist for controller
modal-state, offset/setup-state, and program-dependency evidence before generated
or imported machine code can move toward release. The catalog calls out required
units, positioning, plane, cutter/tool compensation, datum/work-offset,
tool-length/tool-radius, macro/subprogram, postprocessor-version, tool map, and
dry-run/simulation evidence. Failed preflight checks should be retained through
`POST /fabrication/controllers/result` and `POST /fabrication/machine-code/result`
so MDP/POMDP/neural workers can learn which controller routes are reliable.

## `POST /fabrication/controllers/result`

`POST /controllers/result` and the gateway-prefixed
`POST /fabrication/controllers/result` accept controller/postprocessor worker
reviews for generated printer, mill, router, lathe, sheet-cutting, and special
process outputs. The request uses
`dd.fabrication.controller-postprocessor-result-review.v1` style fields for
controller targets, known-postprocessor status, retained postprocessed output,
dry-run or simulation checks, controller/modal checks, artifacts, warnings, and
review metadata.

The response keeps `machineReady=false` and `releaseBlocked=true` while a
postprocessor is unknown, output is not retained, dry-run or simulation did not
pass, modal/units/offset/macro/kinematic checks fail, retained artifacts lack
URI/checksum/evidence, or a human controller review remains open. Stored
artifacts include `controller-postprocessor-result`,
`controller-postprocessor-targets`, `controller-postprocessor-checks`,
`controller-postprocessor-artifacts`, and
`controller-postprocessor-learning-observations`, feeding MDP/POMDP/neural
learning about reliable postprocessors, manual-review routes, and controller
failure boundaries before machine release.
The `learning.outcomeDraft` uses
`dd.fabrication.controller-postprocessor-learning-outcome-draft.v1` with target,
program, controller, postprocessor, target-status, check, artifact,
human-intervention, blocker, reward, and submit-route hints so retained
controller outputs can be submitted directly to
`POST /fabrication/learning/outcomes`.

## `GET /fabrication/process/catalog`

`GET /process/catalog` and the gateway-prefixed
`GET /fabrication/process/catalog` return the live
`dd.fabrication.process-catalog.v1` operation-sequencing discovery contract for
turning a fabrication request into a retained `processPlan` and `processGraph`.
The catalog covers additive print processes, subtractive machining, sheet cutting
and forming, postprocess/quality/release work, and hybrid split/combine
fabrication.

Each process family names required evidence, process graph nodes, release
blockers, and learning signals. The catalog links `process-plan`,
`process-graph`, `hybrid-make-plan`, `manufacturing-handoff`,
`release-package-plan`, and `mdp-request` artifacts to response surfaces such as
`machineSelection`, `materialPlan`, `toolingPlan`, `fixturePlan`, `qualityPlan`,
`postprocessPlan`, and `interventionMap`. Process catalog entries are
draft operation sequencing contracts, not certified CAM, slicer, controller,
fixture, or quality instructions; machine-ready release remains blocked until
every process graph node has retained design, material, tooling, simulation,
quality, intervention, and release-package evidence.

## `GET /fabrication/materials/catalog`

`GET /materials/catalog` and the gateway-prefixed
`GET /fabrication/materials/catalog` return the live
`dd.fabrication.material-catalog.v1` material and feedstock discovery catalog
derived from the current `default_machines()` fleet and the same
material-machine compatibility rules used by planning and instruction analysis.
The payload exposes material families, family counts, compatible machine IDs and
kinds, process classes, feedstock or stock forms, operation tags, conditioning
requirements, release gates, planning and instruction-analysis route aliases,
and response surfaces such as `materialPlan.routeRequirements`,
`materialPlan.releaseGates`, `validation.failureBoundaries`, `boundarySummary`,
`toolingPlan.releaseBlockers`, and `releasePackagePlan.packages`. Catalog
materials are default planning labels, not certified inventory; machine-ready
release remains blocked until material lot/certificate or operator evidence,
quantity plus scrap proof, machine profile evidence, process conditioning,
simulation or dry-run review, and operator or automation signoff are retained.
Material-machine mismatches emit `material-machine-boundary` signals for
MDP/POMDP/neural workers so future plans can learn when to reroute, split, or
request new material evidence.

The catalog also includes a `materialReadinessChecklist` for lot/certificate
traceability, conditioning and shelf-life state, quantity/scrap/runout capacity,
and machine-material-process compatibility. Each entry names blocked surfaces
such as `materialPlan.routeRequirements`, `releasePackagePlan.packages`,
`machineRelease.blockers`, `executionPlan.stopPoints`, and
`machineSchedule.dependencyHolds`, plus learning signals such as
`material-lot:*`, `material-conditioning:*`, `runout-risk:*`, and
`material-machine-boundary:*`.

## `POST /fabrication/materials/plan`

`POST /materials/plan` and the gateway-prefixed
`POST /fabrication/materials/plan` accept the same request body as
`POST /fabrication/plan`, apply bounded learning-policy memory, run the planner,
store and publish the full plan result, and return a compact
`dd.fabrication.material-planning.v1` material-readiness package. The response
focuses on `materialPlan.status`, `materialPlan.material`,
`materialPlan.declaredStock`, `materialPlan.routeRequirements`,
`materialPlan.routeRequirements.feedstockKind`,
`materialPlan.routeRequirements.stockForm`,
`materialPlan.routeRequirements.conditioning`,
`materialPlan.routeRequirements.requiredEvidence`,
`materialPlan.routeRequirements.releaseBlockers`, `materialPlan.releaseGates`,
`materialPlan.learningObservations`, `machineSelection.candidates`,
`toolingPlan.requirements.consumables`, `validation.failureBoundaries`,
`machineRelease.blockers`, and `releasePackagePlan.requiredArtifacts`.

Material plans are draft feedstock, stock, lot, quantity, scrap, conditioning,
and support-media evidence packages, not certified inventory or material
acceptance records. Machine-ready release remains blocked while material
lot/certificate, stock form and dimensions, conditioning, process support media,
machine profile evidence, simulation, or operator signoff is unresolved. Stored
artifacts include `material-plan`, `machine-selection`, `tooling-plan`,
`quality-plan`, `release-package-plan`, `machine-release`, `simulation-report`,
and `mdp-request` so MDP/POMDP/neural workers can learn when to reroute to
another machine, split a part, require conditioning, or request human evidence.
The response also advertises result handoff routes for `POST /materials/result`
and `POST /fabrication/materials/result`.

## `POST /fabrication/materials/result`

`POST /materials/result` and the gateway-prefixed
`POST /fabrication/materials/result` accept material, feedstock, stock,
certificate, conditioning, support-media, and inventory worker results. They
normalize those records into `dd.fabrication.material-result-review.v1`, retain a
bounded review job, and expose release blocker counts for missing certificates,
traceability, moisture, shelf-life, contamination, quantity shortages,
conditioning windows, checks, missing artifact evidence, and human
interventions.

Material result reviews are retained lot and feedstock evidence, not certified
inventory acceptance. Machine-ready release remains blocked until lots,
quantities, certificates, traceability, conditioning, checks, artifacts, and
human dispositions clear. Stored artifacts include `material-result`,
`material-lots`, `material-conditioning`, `material-checks`,
`material-artifacts`, and `material-learning-observations` so
MDP/POMDP/neural workers can learn when to choose alternate feedstock, reroute
machines, split parts, combine assemblies, add conditioning, or require human
evidence before release. The learning section includes a
`dd.fabrication.material-learning-outcome-draft.v1` `outcomeDraft` with lot,
conditioning, check, artifact, blocker, certificate, traceability, quantity,
human-intervention, reward, and submit-route hints ready for
`POST /fabrication/learning/outcomes`.

## `GET /fabrication/design/formats`

`GET /design/formats` and the gateway-prefixed
`GET /fabrication/design/formats` return the live
`dd.fabrication.design-format-catalog.v1` CAD/model/slicer intake catalog before
a caller submits a full planning request. The payload exposes the same
`designInputReview.supportedFormats` source as `/fabrication/plan`, plus source
systems, ecosystems, categories, category counts, preferred neutral exports,
slicer targets, CAD design-conversion NATS request/result subjects, and release
policy notes that keep machine-ready release blocked until translator output,
topology/scale/profile review, simulation, and signoff evidence are attached.

## `GET /fabrication/slicers/catalog`

`GET /slicers/catalog` and the gateway-prefixed
`GET /fabrication/slicers/catalog` return the live
`dd.fabrication.slicer-profile-catalog.v1` print-preparation profile catalog for
PrusaSlicer, OrcaSlicer, Cura, Bambu Studio, Lychee Slicer, and Chitubox. The
payload lists accepted slicer project/profile formats, profile evidence
expectations, generated instruction kinds, release blockers, learning signals,
the `slicer-profile-reviewer` worker lane, design-conversion request/result
subjects, and related generation, validation, simulation, machine-code, and
toolpath routes. Resin slicer entries retain exposure, lift/retract, support,
island, hollowing/drain, peel/recoat, wash/cure, resin lot, and PPE evidence
before machine-ready release.

Slicer catalog entries are accepted profile-evidence contracts, not certified
printer-ready G-code. Machine-ready release remains blocked until profile
provenance, material/nozzle compatibility, support/orientation, first-layer,
simulation, and operator or automation signoff evidence clear. Slicer profile,
support, first-layer, material-map, and kinematic outcomes are retained as
MDP/POMDP/neural learning signals so future print jobs can choose safer slicer
settings.

## `POST /fabrication/slicers/result`

`POST /slicers/result` and the gateway-prefixed
`POST /fabrication/slicers/result` let PrusaSlicer, OrcaSlicer, Cura, Bambu
Studio, Lychee Slicer, Chitubox, and custom print-prep workers report retained
profile, support, first-layer or exposure-stack, generated G-code/resin-job, and
slicer-project evidence back to the fabrication server. The
`dd.fabrication.slicer-profile-result-review.v1`
response reviews profile provenance checks, print-preparation gates,
machine-code checks, retained artifacts, human-intervention requirements, and
learning observations, then stores `slicer-profile-result`,
`slicer-profile-checks`, `slicer-print-preparation`,
`slicer-machine-code-checks`, `slicer-profile-artifacts`, and
`slicer-profile-learning-observations` artifacts.

Machine-ready release remains blocked when slicer workers fail, profile checks
are missing or blocked, support/orientation/first-layer/material-map preparation
is unreviewed, generated printer code checks fail, artifacts lack URI/checksum
evidence, or human intervention is still required. Observations such as
`slicer-profile-check:*`, `slicer-preparation:*`,
`slicer-machine-code-check:*`, and `slicer-profile:release-blocked` feed the
bounded MDP/POMDP/neural policy memory so future print jobs can pick safer
profiles, split fragile parts, lower high-speed settings, or request operator
review earlier.
The `learning.outcomeDraft` uses
`dd.fabrication.slicer-profile-learning-outcome-draft.v1` with slicer,
printer-family, material, profile-check, preparation, machine-code-check,
artifact, human-intervention, blocker, reward, and submit-route hints for direct
submission to `POST /fabrication/learning/outcomes`.

`GET /printers/preflight/catalog` and the gateway-prefixed
`GET /fabrication/printers/preflight/catalog` return the live
`dd.fabrication.printer-preflight-catalog.v1` additive release checklist. The
catalog groups thermal/motion state, extrusion/material/resume state, and
support/orientation/first-article evidence before generated or imported printer
instructions can move toward release. It calls out homing, `M109`/`M190` or
explicit temperature proof, bed mesh/Z-offset evidence, extrusion reset, purge
or prime evidence, material capacity, support/orientation review, first-layer or
first-slice preview, and simulation, dry-run, telemetry, quality, or operator
signoff gates.

Printer preflight entries are additive release evidence contracts, not live
printer approvals. Failed checks should be retained through slicer, material,
quality, telemetry, and learning outcome routes so MDP/POMDP/neural workers can
learn safer print strategies.

## `GET /fabrication/mesh-repair/catalog`

`GET /mesh-repair/catalog` and `GET /fabrication/mesh-repair/catalog` return the
live `dd.fabrication.mesh-repair-catalog.v1` pre-slice geometry repair catalog.
The catalog covers watertight topology repair, scale/unit and wall-thickness
review, color/texture/material preservation, and orientation/support/first-layer
readiness for STL, 3MF, OBJ, PLY, VRML/WRL, glTF/GLB, scan meshes, organic meshes,
and repaired slicer-ready handoffs. Each domain lists required evidence, release
blockers, and learning signals so mesh repair remains a reviewable
design-conversion worker lane, not certified printable geometry.

The response also exposes supported mesh and slicer-source formats from the
design format catalog, the `mesh-repair-converter` worker lane, the design
conversion request/result NATS subjects, related import/conversion/simulation
and validation routes, and response surfaces such as
`designInputReview.reviewedInputs`, `designInputReview.conversionPlan`,
`simulation.riskProfile`, `qualityPlan.inspectionGates`, and
`machineRelease.blockers`. Machine-ready release stays blocked until topology,
scale, wall thickness, color/material preservation, orientation/support,
simulation, and operator or automation signoff evidence clear. Repair outcomes,
dimensional drift, support/orientation choices, and first-layer results are
retained as MDP/POMDP/neural learning signals for later print-prep choices.

## `POST /fabrication/mesh-repair/result`

`POST /mesh-repair/result` and the gateway-prefixed
`POST /fabrication/mesh-repair/result` accept retained mesh-repair worker
outcomes before slicer handoff. The response uses
`dd.fabrication.mesh-repair-result-review.v1` and reviews topology checks,
dimensional drift, wall thickness, orientation/support readiness, split/combine
decisions, human intervention requirements, and retained repair artifacts.

Machine-ready and slicer-ready release remain blocked while repaired geometry
still has topology blockers, dimensional drift over tolerance, thin-wall or
scale issues, unreviewed support/orientation choices, missing split/combine
approval, or artifacts without URI/checksum/evidence labels. The result is
stored with `mesh-repair-result`, `mesh-repair-topology-checks`,
`mesh-repair-dimensional-reviews`, `mesh-repair-orientation-reviews`,
`mesh-repair-artifacts`, and `mesh-repair-learning-observations` artifacts so
future MDP/POMDP/neural planners can choose repair, redesign, split/combine,
support, or human-review paths earlier. The learning section includes a
`dd.fabrication.mesh-repair-learning-outcome-draft.v1` `outcomeDraft` with
topology, dimensional-drift, orientation/support, artifact, split/combine,
redesign, human-intervention, reward, and submit-route hints ready for
`POST /fabrication/learning/outcomes`.

## `GET /fabrication/design/import/catalog`

`GET /formats/catalog`, `GET /design/import/catalog`, and their gateway-prefixed
`GET /fabrication/formats/catalog` / `GET /fabrication/design/import/catalog`
aliases return the live
`dd.fabrication.design-import-catalog.v1` translator and import worker-lane
catalog derived from the same supported CAD/model/slicer source definitions used
by `designInputReview`. The payload maps Creo/Pro/ENGINEER, SOLIDWORKS, Fusion,
Siemens NX, CATIA, Onshape, FreeCAD, OpenSCAD, Blender, ZBrush, JT/PMI,
Parasolid/ACIS, STEP/IGES, DXF/DWG, 3MF/STL/OBJ/color mesh, and slicer project
inputs to worker lanes such as `professional-cad-converter`,
`parametric-cad-converter`, `lightweight-cad-pmi-inspector`,
`cad-kernel-inspector`, `sheet-profile-cad-inspector`,
`mesh-package-inspector`, and `slicer-profile-reviewer`.
It also exposes design-conversion NATS request/result subjects, required source
identity evidence, URI redaction and ambiguous `.prt`/`.asm` policies, review
gates, preferred neutral exports, release blockers, response surfaces such as
`designInputReview.conversionPlan`, and artifact surfaces such as
`design-input-review`, `design-export-bundle`, and `mdp-request`. Machine-ready
release remains blocked until conversion results, topology/scale/profile review,
neutral export checksums, simulation, and operator or automation signoff are
retained.

The same payload includes a `translatorReadinessChecklist` for native CAD
translator provenance, neutral-kernel/PMI preservation, mesh or slicer profile
readiness, and sheet-profile/CAM handoff. Each checklist entry names required
evidence, response or artifact surfaces it blocks, and learning signals such as
`cad-translator:*`, `pmi-preservation:*`, `mesh-readiness:*`, and
`cam-handoff:*` so import failures can train later DES, MDP/POMDP, and neural
routing choices.

## `GET /fabrication/design/preflight/catalog`

`GET /design/preflight/catalog` and the gateway-prefixed
`GET /fabrication/design/preflight/catalog` return the live
`dd.fabrication.design-preflight-catalog.v1` CAD/model/slicer release checklist.
The catalog groups source identity and provenance state, geometry/units/feature
state, and conversion/simulation/learning state before imported Creo/ProE,
SOLIDWORKS, Fusion, NX, CATIA, Onshape, FreeCAD, OpenSCAD, Blender, ZBrush,
JT/PMI, STEP/IGES, Parasolid/ACIS, mesh, scan, or slicer-project sources can
feed design generation, machine-code generation, or machine-ready release. It
calls out file/source-system identity, translator version evidence, ambiguous
`.prt`/`.asm` disambiguation, checksum and revision retention, units and scale,
assembly transforms, watertightness, wall thickness, PMI/GD&T, material/color
metadata, neutral export comparison, mesh repair, manufacturability,
split/combine review, simulation, first-article, metrology, and operator or
automation signoff gates.

Design preflight entries are evidence contracts, not certified geometry or
translator approvals. Failed checks should be retained through design import,
conversion, mesh repair, quality, and learning outcome routes so DES,
MDP/POMDP, and neural workers can learn safer translators, split/combine
strategies, and human-review boundaries.

## `POST /fabrication/design/import/review`

`POST /design/import/review` and the gateway-prefixed
`POST /fabrication/design/import/review` run the same bounded
`designInputs` validation used by `/fabrication/plan` without generating a full
fabrication plan. The request body accepts an optional `requestId` and
`designInputs` array with `fileName`, `sourceUri`, `format`, `sourceSystem`,
`role`, and `notes`; each entry must still include a source identity field, and
source URIs are retained without userinfo, query strings, or fragments.
The response returns `dd.fabrication.design-import-review.v1`,
`designInputReview.inputs`, `designInputReview.conversionPlan`, worker lanes,
target neutral exports, design-conversion NATS request/result subjects, release
blocker counts, and the same notes-only/ambiguous `.prt` and `.asm` policies as
the planner. It is an import review and dispatch contract, not a geometry
certification route: `machineReady` remains false until translator/export
results, topology/scale/profile review, neutral export checksums, simulation, and
operator or automation signoff are attached back to the plan or release package.

## `POST /fabrication/design/import/result`

`POST /design/import/result` and the gateway-prefixed
`POST /fabrication/design/import/result` let external CAD, mesh, scan, and
slicer-project import workers report retained validation evidence back to the
fabrication server. The `dd.fabrication.design-import-result-review.v1` response
reviews topology, unit/scale, PMI/profile, and manufacturability checks; records
failure boundaries that require human intervention, split/combine, conversion,
or regeneration; verifies retained artifacts have URI, checksum, format, and
evidence; and stores `design-import-result`, `design-import-checks`,
`design-import-failure-boundaries`, `design-import-artifacts`, and
`design-import-learning-observations` artifacts. The learning section also emits
a `dd.fabrication.design-import-learning-outcome-draft.v1` `outcomeDraft` with
worker-lane, source-format, check, boundary, recommended-action, artifact,
split/combine, human-intervention, and blocker hints ready for
`POST /fabrication/learning/outcomes`.

Machine-ready release remains blocked when import workers fail, checks are
missing or blocked, failure boundaries remain unresolved, split/combine or human
review is required, or artifact evidence is incomplete. Learning observations
such as `design-import-check:*`, `design-import-boundary:*`,
`design-import-action:*`, and `design-import:split-required` feed the bounded
MDP/POMDP/neural policy memory so future jobs can pick safer translators, split
or repair geometry earlier, or request human review before toolpath generation.

## `POST /fabrication/design/convert/plan`

`POST /design/convert/plan` and the gateway-prefixed
`POST /fabrication/design/convert/plan` run the same bounded CAD/model/slicer
input review as `/fabrication/design/import/review`, then promote
`designInputReview.conversionPlan` to a top-level
`dd.fabrication.design-conversion-plan.v1` worker dispatch package. The response
includes `conversionPlan`, `conversionStepCount`, `conversionStatus`,
`dispatchReady`, and `workerDispatch` with the design-conversion NATS request,
result, and queue-group subjects. It is meant for Creo/Pro/ENGINEER,
SOLIDWORKS, Fusion, NX, CATIA, Onshape, FreeCAD, OpenSCAD, Blender, ZBrush,
JT/PMI, STEP/IGES, 3MF/STL/OBJ, and slicer project intake workers that need a
compact conversion envelope before CAD/CAM, slicer, or release-package work.
Machine-ready release remains blocked until worker conversion results, neutral
export checksums, topology/scale/profile review, simulation, and operator or
automation signoff are retained in `designInputReview`, `designExports`,
`machineRelease`, `releasePackagePlan`, and MDP/POMDP/neural learning surfaces.

## `POST /fabrication/design/convert/result`

`POST /design/convert/result` and the gateway-prefixed
`POST /fabrication/design/convert/result` normalize worker results from
`dd.remote.fabrication.design.conversion.results` back into fabrication release
evidence. The request can name the source job, design input, source format,
source system, worker lane, conversion status, neutral exports, evidence,
blockers, notes, and optional metadata. The response returns
`dd.fabrication.design-conversion-result-review.v1` with `conversionResult`,
`releaseUpdate`, `releaseBlocked`, `missingReleaseEvidence`,
`neutralExportCount`, and MDP/POMDP/neural learning observations. Successful
converter output is still review evidence rather than certified geometry:
machine-ready release remains false until neutral export checksums, units,
topology/scale/profile review, simulation, and operator or automation signoff
are retained in `designInputReview`, `designExports`, `machineRelease`, and
`releasePackagePlan`.
The `learning.outcomeDraft` uses
`dd.fabrication.design-conversion-learning-outcome-draft.v1` with input,
source-format, source-system, worker-lane, status, converted, neutral-export,
blocker, evidence, reward, and submit-route hints for direct submission to
`POST /fabrication/learning/outcomes`.

## `GET /fabrication/design/generation/catalog`

`GET /design/generation/catalog` and the gateway-prefixed
`GET /fabrication/design/generation/catalog` return the live
`dd.fabrication.design-generation-catalog.v1` design package, generated export,
and manufacturing handoff catalog before a caller asks `/fabrication/plan` to
draft geometry or downstream instructions. The payload exposes export contracts,
handoff contracts, export formats, planning route aliases, design-input route
aliases, response surfaces such as `designPackage`, `designExports`,
`designInputReview.conversionPlan`, `manufacturingHandoff.parts`,
`processGraph.nodes`, and `hybridMakePlan.splitCombineDecisions`, plus artifact
surfaces such as `parametric-design`, `design-package`, `design-export-bundle`,
and `manufacturing-handoff`. It names the draft schemas consumed by CAD/CAM,
slicer, setup, MDP-request, and release-package workers, including
`dd.fabrication.design-package.v1`,
`dd.fabrication.design-export-bundle.v1`, and
`dd.fabrication.manufacturing-handoff.v1`. Machine-ready release remains blocked
while generated exports, conversion evidence, machine-release gates, or handoff
proof are unresolved, and design/export/handoff/split-combine observations are
emitted for MDP/POMDP/neural workers.

## `POST /fabrication/design/generate`

`POST /design/generate` and the gateway-prefixed
`POST /fabrication/design/generate` run the same planner as
`/fabrication/plan`, retain the normal plan artifacts, and return a compact
`dd.fabrication.design-generation.v1` design-generation envelope for callers
that want draft geometry and handoff surfaces without the full plan response.
The response highlights `design`, `designPackage`, `designExports`,
`designInputReview`, `manufacturingHandoff`, `processGraph`,
`hybridMakePlan`, `machineSelection`, `generatedPrograms`,
`releasePackagePlan`, and `machineRelease`, plus counts for design parts,
generated design exports, handoff parts, generated programs, and release
blockers. It also carries the local `des_engine` learning surfaces so
MDP/POMDP/neural workers can learn from geometry regeneration, export failure,
split/combine, and machine-selection outcomes. Generated design packages,
native/neutral CAD, mesh, CAM, and slicer export payloads remain deterministic
drafts, not certified machine-ready output, until translator/export evidence,
topology/scale/profile review, simulation, setup, quality, release-package, and
operator or automation signoff gates clear.

## `POST /fabrication/design/synthesis/result`

`POST /design/synthesis/result` and the gateway-prefixed
`POST /fabrication/design/synthesis/result` normalize worker results from
`dd.remote.fabrication.design.synthesis.results` into
`dd.fabrication.design-synthesis-result-review.v1`. External design workers can
return generated candidates, accepted candidate IDs, parametric or neutral source
artifact URIs, export formats, manufacturing method hints, manufacturability
evidence, blockers, notes, and metadata. The response reports
`designSynthesisResult`, `releaseUpdate`, `releaseBlocked`, `candidateCount`,
`missingReleaseEvidence`, and MDP/POMDP/neural learning observations. These
results are draft design evidence rather than certified CAD/CAM/slicer geometry:
machine-ready release remains false until an accepted candidate, source
artifacts, export checksums, manufacturability review, simulation, and operator
or automation signoff are retained in `designPackage`, `designExports`,
`manufacturingHandoff`, `machineRelease`, and `releasePackagePlan`.
The `learning.outcomeDraft` uses
`dd.fabrication.design-synthesis-learning-outcome-draft.v1` with worker-lane,
accepted-candidate, candidate, part, export-format, manufacturing-method,
candidate-review, manufacturability-evidence, blocker, reward, and submit-route
hints so split/combine and hybrid manufacturing candidates can be submitted to
`POST /fabrication/learning/outcomes`.

## `GET /fabrication/handoff/catalog`

`GET /handoff/catalog` and the gateway-prefixed
`GET /fabrication/handoff/catalog` return the live
`dd.fabrication.handoff-catalog.v1` downstream worker-lane catalog for moving
draft design, machine-code, setup, release, and learning evidence through CAD,
CAM, slicer, controller, operator, inspection, monitoring, assembly, and policy
workers. The payload exposes handoff lanes for source CAD/model/slicer
conversion, generated design and CAM exports, machine-program/controller release,
setup-quality-monitoring release, hybrid split/combine assembly, and
learning-policy/outcome feedback. It names source surfaces such as
`designInputReview.conversionPlan`, `designPackage`, `designExports`,
`manufacturingHandoff.parts`, `generatedPrograms`, `controllerPlan`,
`postprocessPlan`, `releasePackagePlan.packages`, `toolingPlan`, `fixturePlan`,
`qualityPlan`, `monitoringPlan`, `hybridMakePlan`, `decompositionPlan`,
`interfaceControlPlan`, `mdp-request`, and `learning.outcomes`, plus artifact
surfaces such as `design-export-bundle`, `generated-design-export`,
`manufacturing-handoff`, `program-*`, `controller-plan`, `postprocess-plan`,
`release-package-plan`, `monitoring-plan`, `interface-control-plan`,
`decomposition-plan`, and `mdp-request`. Handoff lanes are worker contracts, not
certified CAD, CAM, controller, fixture, inspection, or safety-system output;
machine-ready release remains blocked while conversion, export, controller,
setup, monitoring, split/combine, release-package, or learned-remediation
evidence is unresolved.

## `POST /fabrication/handoff/result`

`POST /handoff/result` and the gateway-prefixed
`POST /fabrication/handoff/result` accept downstream worker handoff-review
results after CAD, CAM, slicer, setup, split/combine assembly, transport,
inspection, release, or learning lanes finish a handoff attempt. The request
uses `dd.fabrication.handoff-result-review.v1` style fields for handoff
segments, datum/interface transfers, transport or operator holds, retained
artifacts, reviewer metadata, and warnings. The response blocks release when a
segment is rejected, rework is required, a datum or interface is unverified,
transport/cure/queue holds remain open, or retained artifact URI/checksum/evidence
is missing.

Stored review artifacts include `handoff-result`, `handoff-segments`,
`handoff-datum-transfers`, `handoff-transport-holds`, `handoff-artifacts`, and
`handoff-learning-observations`, so MDP/POMDP/neural policy workers can learn
which worker lanes, datum transfers, split/combine interfaces, queue holds, and
human interventions blocked or enabled machine-ready release. The learning
section includes a `dd.fabrication.handoff-learning-outcome-draft.v1`
`outcomeDraft` with segment, datum-transfer, transport-hold, artifact, rework,
human-intervention, reward, and submit-route hints ready for
`POST /fabrication/learning/outcomes`.

## `GET /fabrication/subjects/catalog`

`GET /subjects/catalog` and the gateway-prefixed
`GET /fabrication/subjects/catalog` return the live
`dd.fabrication.subject-catalog.v1` NATS worker-dispatch contract for external
CAD/CAM, slicer, postprocessor, simulator, assembly, execution, release, and
learning workers. The payload lists the primary fabrication request/result
subjects, queue group, runtime event subject, MDP optimize subject, environment
override names, and per-lane request/result subjects for design conversion,
design synthesis, instruction generation, instruction review, instruction
simulation, assembly planning, execution telemetry, and release readiness.

Subject catalog entries are dispatch contracts, not guaranteed worker
availability. Worker result subjects carry retained evidence for generated
designs, machine code, simulations, split/combine assembly, execution telemetry,
release packages, and learning outcomes; machine-ready release remains blocked
until worker outputs, validation, setup, simulation, quality, operator or
automation signoff, and release gates clear.

## `GET /fabrication/workers/catalog`

`GET /workers/catalog` and the gateway-prefixed
`GET /fabrication/workers/catalog` return
`dd.fabrication.worker-catalog.v1`, a worker-facing view of the same dispatch
lanes exposed by the subject catalog. The response lists worker lanes, queue
groups, request/result subjects, payload and response families, result-review
requirements, retained-evidence requirements, and review routes such as design
conversion, instruction generation, simulation, release readiness, and learning
optimizer result review. Worker catalog entries are dispatch and integration
contracts only; generated designs, machine code, simulations, release packages,
and learning artifacts remain blocked until their retained worker results clear
the normal validation, setup, simulation, quality, telemetry, operator or
automation, and release gates.

## `GET /fabrication/results/catalog`

`GET /results/catalog` and the gateway-prefixed
`GET /fabrication/results/catalog` return
`dd.fabrication.result-review-catalog.v1`, a compact inventory of the worker
result-review intake routes that normalize CAD/model/slicer conversion,
instruction generation and review, simulation, split/combine assembly,
execution, shop-support, quality, telemetry, release, and learning optimizer
outputs. The response links each review family to retained evidence
expectations, result subjects from the subject catalog, worker catalog routes,
job evidence routes, and learning outcome routes.

The catalog is discovery metadata only. Reviewed worker results become job,
artifact, release, telemetry, or learning evidence only after checksums,
findings, blocker state, and required artifacts are retained; machine-ready
release remains blocked until the relevant design, instruction, setup,
simulation, quality, operator or automation, and final release reviews clear.

## `GET /fabrication/instructions/languages`

`GET /instructions/languages` and the gateway-prefixed
`GET /fabrication/instructions/languages` return the live
`dd.fabrication.instruction-language-catalog.v1` intake catalog for imported CNC,
CAM intermediate, printer, slicer, cutting, EDM, assembly, part-separation,
setup, and operator instruction streams before a caller submits analysis or planning work. The
payload exposes language families, family counts, machine classes, analysis
focus areas, analysis route aliases, draft-only release policy notes, and
per-language release gates. Controller dialect languages such as
`siemens-sinumerik`, `heidenhain`, `heidenhain-conversational`, `mazatrol`,
`mazak-mazatrol`, `okuma-osp`, and `linuxcnc` are advertised as imported
machine-code/controller streams for mills, routers, lathes, five-axis machines,
and mill-turn centers while retaining controller-specific modal-state,
postprocessor, dry-run, and operator or automation review gates. The deterministic
controller labels map to `siemens-sinumerik-postprocessor`,
`heidenhain-conversational-postprocessor`, `mazatrol-conversational-postprocessor`,
`okuma-osp-postprocessor`, or `linuxcnc-gcode-postprocessor` before release
evidence is reviewed. Resin package languages such as `ctb-resin-job`,
`photon-resin-job`, `lychee-resin-job`, and `chitubox-resin-job` are advertised as
SLA/MSLA resin-printer intake so Lychee/Chitubox/Photon/CTB slice packages can
carry exposure image stack, peel/lift/recoat, resin profile, wash/cure, resin lot,
and PPE release evidence. CAM intermediate languages such as `apt-cldata`,
`apt-source`, `cldata-toolpath`, `cutter-location-file`, `postprocessor-deck`,
and `cam-intermediate-job` are advertised separately from controller G-code so
APT/CLDATA, cutter-location, and postprocessor deck handoffs retain source
CAD/CAM setup identity, tool table, tool-axis/contact-point, controller target,
translated program, dry-run, and operator or automation release evidence.
Machine-ready release remains blocked until the submitted instruction stream has
parse or review evidence, simulation or equivalent controller review when
machine code is present, improved program or patch retention when repairable,
and controller/postprocess/operator signoff evidence for any machine-failure,
human-intervention, split/combine, setup, or handoff boundary.

## `GET /fabrication/cnc/catalog`

`GET /cnc/catalog` and the gateway-prefixed `GET /fabrication/cnc/catalog`
return the live `dd.fabrication.cnc-catalog.v1` intake and generation catalog
for imported CNC/controller programs. The payload filters the default machine
fleet to CNC-capable mills, routers, lathes, mill-turn and Swiss turning
centers, laser/waterjet/plasma/wire EDM sheet cutters, sinker EDM, and other
controller-driven cutting profiles, then exposes machine kinds, controllers,
accepted instruction languages, postprocessors, import-review evidence,
analysis routes, machine-code/toolpath generation routes, result-review routes,
failure-boundary families, and release policy.

The catalog ties imported CNC and generated machine-code workflows together:
modal state, controller macro/subprogram dependencies, arc-plane and center
offsets, tool length and cutter compensation, workholding/datum, spindle/feed,
coolant/chip/fume/support-media, canned-cycle/threading, and tool-change or
operator-intervention risks all keep `machineReady=false` until validation,
simulation or dry-run, controller/postprocessor, setup, release, and operator or
automation evidence clear. CNC validation and result reviews feed DES,
MDP/POMDP, and neural learning surfaces so future plans can split parts, reroute
machines, regenerate code, or require human intervention earlier.

## `GET /fabrication/instructions/review-pipeline/catalog`

`GET /instructions/review-pipeline/catalog` and the gateway-prefixed
`GET /fabrication/instructions/review-pipeline/catalog` return the
`dd.fabrication.instruction-review-pipeline-catalog.v1` stage order for generated
or imported instruction streams. The catalog tells clients to discover language
and machine context, retain the original import or generated artifact, validate
and find machine-failure/human-intervention/split-combine boundaries, improve or
route for human review, then simulate, release-preview, and record learning
outcomes.

Each pipeline stage names route handoffs, required evidence, and blocked
surfaces such as `instructionImportReview.originalPrograms`,
`generatedPrograms`, `validation.failureBoundaries`, `boundarySummary`,
`operatorInterventionPlan.requiredOperatorActions`, `improvements`,
`improvedPrograms.patchManifest`, `simulation.failureBoundaries`,
`releasePackagePlan.releaseGates`, and `learning.outcomes`. The decision rules
make the original stream immutable review evidence: improved programs are patch
drafts, `machineReady` remains false while validation, simulation, controller,
setup, quality, split/combine, operator, or release gates are open, and patch
outcomes feed DES/MDP/POMDP/neural learning through retained feature maps and
reward terms.

## `GET /fabrication/instructions/import/catalog`

`GET /instructions/import/catalog` and the gateway-prefixed
`GET /fabrication/instructions/import/catalog` return the live
`dd.fabrication.instruction-import-catalog.v1` intake contract for existing CNC,
slicer, printer, CAM intermediate, setup-sheet, postprocess, assembly, quality,
and operator instruction streams. The catalog groups controller/CAM machine
code, slicer and additive job files, subtractive/sheet/EDM/special-process job
sheets, and operator assembly/postprocess/quality work. Each group lists
accepted families, examples such as `apt-cldata`, `marlin-gcode`,
`ctb-resin-job`, `wire-edm-job`, and `assembly-cell-job`, required provenance
and release evidence, and release blockers.

Imported instructions are accepted as review inputs, not trusted machine-ready
code. The import contract points callers to `POST /fabrication/instructions/analyze`,
`POST /fabrication/instructions/import/review`,
`POST /fabrication/instructions/validate`, and
`POST /fabrication/instructions/improve`, and names response surfaces including
`validation.failureBoundaries`, `improvements`,
`improvedPrograms.patchManifest`, `operatorInterventionPlan`, simulation,
release-package, and learning surfaces. Machine-ready release remains blocked
until provenance, checksum, dialect, machine profile, validation, simulation or
dry-run, setup, quality, release-package, and operator or automation signoff
evidence clear.

`POST /instructions/import/review` and the gateway-prefixed
`POST /fabrication/instructions/import/review` wrap submitted instruction streams
into the `dd.fabrication.instruction-import-review.v1` review packet. The response
retains original programs, runs the same validation and boundary analyzer used by
`POST /fabrication/instructions/analyze`, counts imported language and machine
families, surfaces validation, simulation, machine-failure, human-intervention,
and split/combine blockers, and keeps `machineReady=false` until release evidence
clears. The packet includes `packageActions` for retaining original instruction
artifacts, running validation/boundary review, reviewing generated patch drafts,
and submitting a learning outcome. It also carries an
`instruction-import-learning-outcome-draft.v1` draft for
`POST /fabrication/learning/outcomes` so MDP/POMDP/neural workers can learn from
imported controller streams, patch decisions, and rejected release attempts.

## `GET /fabrication/instructions/import/preflight/catalog`

`GET /instructions/import/preflight/catalog` and the gateway-prefixed
`GET /fabrication/instructions/import/preflight/catalog` return the live
`dd.fabrication.instruction-import-preflight-catalog.v1` evidence checklist for
existing CNC, slicer, printer, setup-sheet, postprocess, assembly, quality, and
operator instruction streams before they are trusted as fabrication work. The
catalog groups source provenance/language/artifact state,
machine/controller/setup/process state, and
analysis/validation/simulation/improvement/learning state so callers can see
which evidence must be present before imported programs move toward release.

Imported programs remain review inputs until checks for provenance, checksum,
dialect, machine and controller profile, setup and process state, analysis,
validation, simulation or dry-run, improvement deltas, release package, and
operator or automation signoff all clear. `machineReady` remains false while any
preflight gate is missing, and failed checks can feed DES, MDP/POMDP, and neural
learning workers to improve future split/combine, machine-selection,
postprocessor, and intervention planning.

## `GET /fabrication/instructions/validation/catalog`

`GET /instructions/validation/catalog` and the gateway-prefixed
`GET /fabrication/instructions/validation/catalog` return the live
`dd.fabrication.instruction-validation-catalog.v1` contract for generated and
imported CNC, printer, slicer, setup-sheet, postprocess, assembly, and operator
instruction streams before a caller submits validation work. The payload exposes
accepted language families, boundary families, validation check contracts,
response surfaces, artifact surfaces, and learning surfaces.

`GET /instructions/validation/preflight/catalog` and
`GET /fabrication/instructions/validation/preflight/catalog` expose the
`dd.fabrication.instruction-validation-preflight-catalog.v1` checklist for the
same streams before they are trusted as validation inputs. The preflight payload
groups source provenance/language/dialect state, machine/process/simulation
setup state, and boundary/improvement/release/learning state. It names the
`validation.failureBoundaries`, `operatorInterventionPlan`,
`improvedPrograms.patchManifest`, `instruction-validation-result`, and
learning-signal surfaces that keep machine-ready release blocked while evidence
is missing.

The same payload includes a `streamReadinessMatrix` for imported CNC/controller
programs, generated machine code, additive slicer or printer G-code, non-G-code
job sheets or operator instructions, and hybrid split/combine instruction
packages. Each stream entry lists required provenance, dialect, setup,
simulation, human-intervention, recomposition, and release evidence plus blocked
surfaces such as `controllerPlan.releaseGates`, `postprocessPlan.controllerTargets`,
`interventionMap.splitCombineDecisions`, `interfaceControlPlan.releaseGates`,
and `machineRelease.blockers`.

Catalog contracts cover controller modal state, additive printer
heat/extrusion/material state, subtractive spindle/feed/tool/workholding state,
sheet-cutting and EDM process-media state, text job-sheet evidence, and
split/combine release review. They are discovery contracts, not controller
certification: validated programs remain `machineReady=false` while validation
findings, simulation findings, machine-failure boundaries, human-intervention
gates, split/combine reviews, or release blockers remain open. Validation
findings, boundary kinds, instruction patches, DES instruction models, neural
corpus examples, and retained learning outcomes remain available for
MDP/POMDP/neural workers to improve future program generation, machine
selection, and split/combine decisions.

## `GET /fabrication/instructions/generation/catalog`

`GET /instructions/generation/catalog` and the gateway-prefixed
`GET /fabrication/instructions/generation/catalog` return the live
`dd.fabrication.instruction-generation-catalog.v1` generated machine-program and
job-sheet catalog for plan responses. The payload exposes generated program
families for FDM printing, resin and powder-bed additive, pellet FGF, robotic/gantry additive, paste/clay
extrusion, bound-metal filament FFF, material jetting, DED/WAAM, continuous-fiber, composite layup/vacuum-bag/autoclave, hot-wire foam cutting, binder jet, vertical/horizontal/indexed
milling, routing, laser/waterjet/plasma/wire EDM sheet cutting, sinker EDM,
precision grinding/lapping/honing, CMM/vision dimensional inspection, thermal postprocess furnace/oven release, surface finishing/coating/plating/anodizing/media-blasting/powder-coating/deburr release, metal-joining/welding/brazing/soldering release, molding/casting/vacuum-casting/urethane/silicone release, fixture tooling/soft jaws/drill jigs/vacuum fixtures, closed-loop machining/adaptive compensation/in-process probing/inspection feedback/offset and tool-wear updates, insert installation/threaded inserts/heat-set inserts/press-fit bushings/helicoils/dowel pins, adhesive bonding/structural adhesives/epoxy bonding/bondline control/cure and coupon release, plastic joining/ultrasonic welding/heat staking/solvent/hot-plate/vibration/spin welding with energy-director, weld-collapse, proof, leak, visual, and release evidence, fastener installation/mechanical fastening/screw and bolt torque/threadlocker/witness-mark/retorque release, rivet installation/blind-rivet/solid-rivet/clinch/stake/swage/peen release, seal/gasket/O-ring/RTV installation with leak/pressure/vacuum release, bearing/interference-fit/shrink-fit installation with preload/runout/rotation release, dynamic balancing/rotor-balancing/impeller-balancing/fan-balancing release with residual-unbalance/vibration evidence, composite layup/prepreg/wet-layup/vacuum-bag/autoclave/resin-infusion release, hot-wire foam cutting release, press-brake/sheet-forming release, gear/spline cutting release, lathe, mill-turn, Swiss/sliding-headstock turning, robotic assembly, part separation, and fallback manual
instructions. It lists generated languages such as `marlin-gcode`,
`ctb-resin-job`, `photon-resin-job`, `lychee-resin-job`,
`chitubox-resin-job`, `haas-gcode`, `indexed-mill-gcode`, `waterjet-job`, `wire-edm-job`,
`grinding-job`, `cmm-inspection-job`, `vision-inspection-job`, `metrology-job`,
`thermal-postprocess-job`, `furnace-job`, `heat-treatment-job`,
`surface-finishing-job`, `coating-job`, `plating-job`, `anodizing-job`,
`media-blasting-job`, `powder-coating-job`, `deburr-polish-job`,
`metal-joining-job`, `welding-job`, `brazing-job`, `soldering-job`,
`molding-casting-job`, `mold-casting-job`, `casting-job`, `molding-job`,
`urethane-casting-job`, `silicone-molding-job`, `vacuum-casting-job`,
`injection-molding-job`,
`pcb-fabrication-job`, `pcb-fab-job`, `gerber-fabrication-job`,
`isolation-milling-job`, `excellon-drill-job`,
`pcb-assembly-job`, `electronics-assembly-job`, `smt-assembly-job`,
`pick-and-place-job`, `reflow-job`,
`fixture-tooling-job`, `soft-jaw-job`, `fixture-plate-job`, `drill-jig-job`,
`inspection-fixture-job`, `assembly-fixture-job`, `vacuum-fixture-job`,
`adaptive-compensation-job`, `closed-loop-machining-job`,
`in-process-probing-job`, `inspection-feedback-job`, `offset-update-job`,
`tool-wear-update-job`, `compensated-rerun-job`,
`insert-installation-job`, `threaded-insert-job`, `heat-set-insert-job`,
`press-fit-insert-job`, `helicoil-installation-job`,
`dowel-pin-installation-job`, `bushing-installation-job`,
`adhesive-bonding-job`, `structural-adhesive-job`, `epoxy-bonding-job`,
`bondline-control-job`, `adhesive-cure-job`, `lap-shear-peel-test-job`,
`plastic-joining-job`, `plastic-welding-job`, `ultrasonic-welding-job`,
`heat-staking-job`, `solvent-welding-job`, `hot-plate-welding-job`,
`vibration-welding-job`,
`fastener-installation-job`, `mechanical-fastening-job`,
`screw-installation-job`, `bolt-installation-job`, `torque-sequence-job`,
`threadlocker-job`, `retorque-inspection-job`,
`rivet-installation-job`, `blind-rivet-job`, `solid-rivet-job`,
`clinch-stake-job`, `swage-peen-job`, `rivet-inspection-job`,
`seal-installation-job`, `gasket-installation-job`,
`oring-installation-job`, `o-ring-installation-job`, `rtv-sealant-job`,
`leak-test-job`, `pressure-test-job`,
`bearing-installation-job`, `bearing-press-job`,
`interference-fit-job`, `shrink-fit-job`, `bearing-preload-job`,
`runout-check-job`, `rotation-torque-job`,
`dynamic-balancing-job`, `rotor-balancing-job`,
`impeller-balancing-job`, `fan-balancing-job`, `wheel-balancing-job`,
`spin-balance-job`, `vibration-analysis-job`,
`part-marking-job`, `laser-marking-job`, `laser-engraving-job`,
`dot-peen-job`, `data-matrix-marking-job`, `udi-marking-job`,
`packaging-labeling-job`, `packout-job`, `carton-packout-job`,
`serialization-label-job`, `shipment-release-job`,
`composite-layup-job`, `wet-layup-job`, `prepreg-layup-job`,
`vacuum-bag-job`, `autoclave-cure-job`, `resin-infusion-job`,
`robotic-additive-job`, `robotic-pellet-job`, `robotic-extrusion-job`,
`hot-wire-foam-job`, `hot-wire-job`, `foam-cutting-job`, `foam-core-job`,
`wing-core-job`, `press-brake-job`, `sheet-forming-job`, `bend-job`, `gear-cutting-job`,
`gear-hobbing-job`, `spline-broaching-job`, `mill-turn-gcode`,
`swiss-turning-gcode`, `swiss-turning-job`, and `assembly-cell-job`, plus response surfaces including
`generatedPrograms.instructions`, `generatedPrograms.machineReady`,
`simulation.programs`, `executionPlan.programRuns`,
`postprocessPlan.targets`, and `controllerPlan.controllerTargets`. Generated
program artifacts are retained as `generated-machine-program` / `program-*`
records and linked into `mdp-request` artifacts for downstream simulation,
release, controller, and policy workers. Generated programs stay `draft=true`
and `machineReady=false` until validation, simulation or dry-run evidence,
controller/postprocessor review, fixture/material/profile proof, and operator
or automation signoff clear. Program generation observations feed
MDP/POMDP/neural workers so future plans can choose alternate machines,
split/combine parts, regenerate programs, or add human checkpoints.

## `GET /fabrication/instructions/generation/preflight/catalog`

`GET /instructions/generation/preflight/catalog` and the gateway-prefixed
`GET /fabrication/instructions/generation/preflight/catalog` return the
`dd.fabrication.instruction-generation-preflight-catalog.v1` evidence checklist
for draft generated machine programs and job sheets before validation or release
review. The catalog groups preflight requirements into request/design/machine
state, program draft/controller state, and validation/simulation/release/learning
state.

Generation preflight entries are review gates, not controller-certified machine
instructions. Generated programs stay `draft=true` and `machineReady=false` until
design or import evidence, machine and controller profile evidence, setup and
material/process context, validation, simulation or dry-run evidence, quality and
release gates, and operator or automation signoff clear. Failed checks feed DES,
MDP/POMDP, and neural workers so future plans can choose alternate machines,
split/combine parts, regenerate programs, or add human checkpoints earlier.

## `POST /fabrication/instructions/generate`

`POST /instructions/generate` and the gateway-prefixed
`POST /fabrication/instructions/generate` accept the same request body as
`POST /fabrication/plan`, apply the current bounded learning-policy memory,
generate the deterministic draft program set, retain the normal plan artifacts,
and publish the normal plan outputs when NATS is configured. The compact
`dd.fabrication.instruction-generation.v1` response focuses on
`generatedPrograms`, instruction line counts, `designExports`,
`manufacturingHandoff`, controller and postprocess targets,
`executionPlan.programRuns`, `simulation.programs`, `machineRelease.blockers`,
and `releasePackagePlan.packages`. Generated instruction packages keep
`draft=true` and `machineReady=false` until validation, simulation or dry-run
evidence, controller/postprocessor review, setup, quality, release package, and
operator or automation signoff gates clear.

## `POST /fabrication/instructions/generation/result`

`POST /instructions/generation/result` and the gateway-prefixed
`POST /fabrication/instructions/generation/result` normalize external worker
results from `dd.remote.fabrication.instructions.generation.results` back into a
compact `dd.fabrication.instruction-generation-result-review.v1` review package.
The endpoint accepts generated machine code, slicer jobs, setup sheets,
simulation reports, inspection plans, postprocess travelers, or operator
instructions with retained artifact URI, checksum, evidence labels, blockers,
warnings, worker id, generator, and review metadata.

The response exposes `instructionGenerationResult`, `generationResultJobId`,
`generatedAtMs`, `releaseUpdate`, `releaseBlocked`, `missingReleaseEvidence`,
the request/queue/result subjects for the instruction-generation worker lane,
and a `dd.fabrication.instruction-generation-learning-outcome-draft.v1` payload
with generator, artifact format, target, blocker, evidence, reward, and
submit-route hints for `POST /fabrication/learning/outcomes`. Successful reviews are retained in the bounded job ledger under
`generationResultJobId`; `/jobs/:job_id` and
`/jobs/:job_id/artifacts/:artifact_id` can inspect
`instruction-generation-result`, `instruction-generation-artifacts`,
`instruction-generation-blockers`, `instruction-generation-warnings`,
`instruction-generation-release-update`, and
`instruction-generation-learning-observations`. Machine-ready release remains
blocked until generated artifacts are retained with checksums, worker evidence
is attached to controller/slicer/setup/simulation/inspection targets, blockers
are folded into `machineRelease`, `executionPlan`, and `releasePackagePlan`, and
operator or automation signoff clears any human-intervention or split/combine
boundary. Result observations feed MDP/POMDP/neural workers so future plans can
prefer reliable CAM, slicer, postprocessor, simulation, and setup-sheet workers.

## `POST /fabrication/instructions/review/result`

`POST /instructions/review/result` and the gateway-prefixed
`POST /fabrication/instructions/review/result` normalize external validation,
boundary-analysis, and instruction-improvement worker results from
`dd.remote.fabrication.instructions.review.results` back into a compact
`dd.fabrication.instruction-review-result-review.v1` package. The endpoint
accepts validation findings, failure boundaries, improvement drafts, worker and
reviewer identity, warnings, and metadata for imported CNC, printer, slicer,
setup-sheet, postprocess, assembly, or operator instruction streams.

The response exposes `instructionReviewResult`, `reviewResultJobId`,
`generatedAtMs`, `releaseUpdate`, `releaseBlocked`, `findingCount`,
`failureBoundaryCount`, `humanInterventionBoundaryCount`,
`improvementDraftCount`, `humanApprovalDraftCount`, and the
request/queue/result subjects for the instruction-review worker lane. It also
includes `priorityDispositions` rows that mirror
`instructionIntentMap.reviewPriorities`, showing whether machine-failure,
human-intervention, split/combine or interface, non-G-code job-sheet, and
learning-feedback review queues are closed, partially reviewed, blocked, or ready
to submit as learning feedback. The
`dd.fabrication.instruction-review-learning-outcome-draft.v1` payload carries the
same priority-disposition rows
with reviewer, finding, boundary, improvement, human-approval, recommended-action,
reward, and submit-route hints for `POST /fabrication/learning/outcomes`. Successful
reviews are retained in the bounded job ledger under `reviewResultJobId`;
`/jobs/:job_id` and `/jobs/:job_id/artifacts/:artifact_id` can inspect
`instruction-review-result`, `instruction-review-findings`,
`instruction-review-failure-boundaries`,
`instruction-review-improvement-drafts`,
`instruction-review-priority-dispositions`, `instruction-review-warnings`,
`instruction-review-release-update`, and
`instruction-review-learning-observations`. Machine-ready release remains
blocked while blocking findings, machine-failure or human-intervention
boundaries, or human-approval improvement drafts remain open. Review
observations include `instruction-review-boundary-kind:*`,
`instruction-review-recommended-action:*`, and
`instruction-review-improvement:*` plus
`instruction-review-priority:<priority>:<disposition>` signals so
MDP/POMDP/neural workers can learn which validators, boundary analyzers, and
repair drafts closed or left open the highest-priority machine-failure,
human-intervention, split/combine, non-G-code evidence, and learning feedback
lanes.

## `GET /fabrication/machine-code/catalog`

`GET /machine-code/catalog` and the gateway-prefixed
`GET /fabrication/machine-code/catalog` return the live
`dd.fabrication.machine-code-catalog.v1` contract for draft machine-code,
controller, slicer, and postprocessor generation before a caller submits a
generation request. The payload derives its program families from the instruction
generation catalog and its controller targets from the controller/postprocessor
catalog, exposing `programContracts`, `controllerTargets`, generated languages,
machine classes, dialect families, output formats, postprocessors, release
evidence, and learning surfaces.

Catalog entries cover printer firmware G-code, CTB/Photon/Lychee/Chitubox resin
package jobs, CAM G-code for vertical mills, horizontal mills, routers, mill-turn
centers, lathes, sheet cutters, EDM and special-process travelers, assembly-cell instructions, and manual fallback
programming requests. They are discovery contracts, not certified controller
output: generated programs remain `draft=true` and `machineReady=false` until
validation, simulation or dry-run evidence, controller/postprocessor
compatibility, setup, quality, release package, and operator or automation
signoff gates clear. Program-generation, controller-release, simulation-risk,
release-probe, neural-corpus, and learning-outcome observations remain available
for MDP/POMDP/neural workers to learn when to regenerate code, reroute machines,
split parts, combine assemblies, or add human checkpoints.

The payload also includes a `targetSelectionMatrix` that maps machine families to
generated languages, postprocessor strategy, preferred handoff routes, release
evidence, and fallback review paths. Matrix entries cover additive printer
firmware, subtractive mill/router controllers including vertical and horizontal
mills, turning and mill-turn controllers including lathes, sheet-cutting/EDM and
special-process outputs, and hybrid assembly or human-reviewed travelers. The
matrix keeps unknown firmware, controller, slicer profile, postprocessor, thermal
state, modal state, part-off support, cut-chart/support-media, or split/combine
interface evidence as release blockers rather than treating generated code as
machine-ready.

## `GET /fabrication/machine-code/preflight/catalog`

`GET /machine-code/preflight/catalog` and the gateway-prefixed
`GET /fabrication/machine-code/preflight/catalog` return the live
`dd.fabrication.machine-code-preflight-catalog.v1` evidence checklist for
generated or imported controller output before machine-code generation, controller
handoff, or release packaging. The catalog groups program source/design state,
controller/postprocessor/dialect state, machine/setup/toolpath/process state, and
validation/simulation/release/learning state.

Machine-code preflight keeps generated programs as `draft=true` and
`machineReady=false` until design provenance, controller/postprocessor
compatibility, machine setup, validation, simulation or dry-run, quality, release
package, and signoff evidence clear. Failed preflight checks feed DES,
MDP/POMDP, reward, neural, and learning-outcome workers so future requests can
regenerate code, choose alternate machines, split/combine parts, or add human
checkpoints before controller release.

## `POST /fabrication/machine-code/generate`

`POST /machine-code/generate` and the gateway-prefixed
`POST /fabrication/machine-code/generate` accept the same request body as
`POST /fabrication/plan`, apply the bounded learning-policy memory, retain the
normal plan artifacts, publish normal plan outputs when NATS is configured, and
return a compact `dd.fabrication.machine-code-generation.v1` envelope for
generated controller, printer, mill, router, sheet-cutting, mill-turn, lathe,
and special-process programs. The response highlights `generatedPrograms`,
program languages, machine kinds, `controllerPlan.compatibilityTargets`,
`controllerPlan.dialectSummaries`, `controllerPlan.releaseGates`,
`postprocessPlan.controllerTargets`, `executionPlan.programRuns`,
`simulation.programs`, `validation.failureBoundaries`,
`machineRelease.generatedProgramsBlocked`, `releasePackagePlan.packages`, and
the local `des_engine` learning surfaces. Machine-code generation remains a
draft controller/postprocessor release package: generated programs keep
`draft=true` and `machineReady=false` until validation, simulation or dry-run,
controller/postprocessor compatibility, setup, quality, release package, and
operator or automation signoff gates clear.

## `POST /fabrication/machine-code/result`

`POST /machine-code/result` and the gateway-prefixed
`POST /fabrication/machine-code/result` normalize controller, postprocessor,
CAM, slicer, setup-sheet, and dry-run worker results into
`dd.fabrication.machine-code-result-review.v1`. The route accepts retained
machine-code programs, controller/postprocessor checks, machine-failure or
human-intervention boundaries, artifacts, warnings, worker identity, controller
identity, and metadata after a worker tries to prove generated code is ready for
the selected machine.

The response exposes `machineCodeResult`, `machineCodeResultJobId`,
`releaseBlocked`, controller-check, boundary, human-intervention, program, and
missing-evidence counts plus follow-up toolpath, simulation, release, and
learning routes. It also includes `priorityDispositions` rows for the
machine-failure, human-intervention, non-G-code or slicer/printer evidence, and
learning-feedback lanes so generated or imported controller output can close or
leave open the same release-priority queues as instruction validation. The
`dd.fabrication.machine-code-learning-outcome-draft.v1` payload carries those
priority dispositions with controller, postprocessor, language, boundary, blocker,
and recommended-action hints that callers can submit to
`POST /fabrication/learning/outcomes`. Review jobs retain
`machine-code-result`,
`machine-code-programs`, `machine-code-controller-checks`,
`machine-code-failure-boundaries`, `machine-code-artifacts`,
`machine-code-priority-dispositions`, and
`machine-code-learning-observations` artifacts for `/jobs/:job_id` inspection.
Machine-ready release stays blocked until controller checks, failure boundaries,
retained URI/checksum evidence, dry-run or simulation evidence, and operator or
automation signoff clear. Observations include `machine-code-program-language:*`,
`machine-code-check:*`, `machine-code-boundary:*`, `machine-code-artifact:*`, and
`machine-code-priority:<priority>:<disposition>` so MDP/POMDP/neural workers can
learn which postprocessors, controllers, and review gates block or clear release.

## `GET /fabrication/toolpaths/catalog`

`GET /toolpaths/catalog` and the gateway-prefixed
`GET /fabrication/toolpaths/catalog` return the live
`dd.fabrication.toolpath-catalog.v1` CAM, slicer, sheet-cut, turning, and hybrid
path evidence catalog before generated or imported work is treated as ready for
machine-code, simulation, setup, execution, or release handoff.

The catalog covers additive slicer/extrusion paths, subtractive rough/finish
paths, turning/threading/part-off and spindle-transfer paths, sheet-cut
nest/kerf/pierce/retention paths, and hybrid split/combine recomposition paths.
Each family lists path evidence, release blockers, response surfaces, artifact
surfaces, and learning signals. Toolpath catalog entries are evidence
contracts, not certified machine programs. Machine-ready release remains blocked
until path geometry, feeds/speeds or process parameters, workholding/datum,
controller modal state, simulation, and operator or automation handoff evidence
clears. Toolpath planning and result observations are retained for
MDP/POMDP/neural workers so future jobs can choose safer machines, split parts,
adjust feeds, add fixtures, or insert human intervention earlier.

## `POST /fabrication/toolpaths/plan`

`POST /toolpaths/plan` and the gateway-prefixed
`POST /fabrication/toolpaths/plan` accept the same request body as
`POST /fabrication/plan`, apply bounded learning-policy memory, retain the
normal plan artifacts, publish normal plan outputs when NATS is configured, and
return a compact `dd.fabrication.toolpath-planning.v1` CAM/slicer/motion
planning envelope. The response links every generated program into a
`toolpathPlan` segment with its `processGraph` node, simulation trace,
controller target, postprocess target, execution run, release package, line
count, operation, and safety notes.

The package highlights `toolpathPlan.simulationTrace`,
`toolpathPlan.controllerTarget`, `toolpathPlan.postprocessTarget`,
`toolpathPlan.releasePackage`, `simulation.riskProfile`,
`machineRelease.blockers`, `releasePackagePlan.packages`,
`learning.releaseProbePlan`, `learning.neuralTrainingCorpus`, and result
handoff routes for `POST /toolpaths/result` and
`POST /fabrication/toolpaths/result`. Toolpath
plans are draft CAM/slicer/controller handoffs for printers, mills, routers,
sheet cutters, mill-turn centers, lathes, and special-process cells; they remain
blocked until CAM/slicer regeneration, simulation or dry-run evidence,
controller/postprocessor compatibility, setup, quality, release package, and
operator or automation signoff clear. Toolpath risk and generated-program
observations feed MDP/POMDP/neural workers so future plans can split parts,
combine assemblies, reroute machines, or regenerate motion.

## `POST /fabrication/toolpaths/result`

`POST /toolpaths/result` and the gateway-prefixed
`POST /fabrication/toolpaths/result` accept CAM, slicer, controller, simulation,
or dry-run worker results, normalize them into
`dd.fabrication.toolpath-result-review.v1`, and store a bounded review job with
retained toolpath segment, simulation, check, artifact, and learning-observation
surfaces. The response reports blocker counts for collision evidence, envelope
excursions, clearance failures, required dry-runs that have not passed,
toolpath checks, human interventions, missing toolpath artifact evidence, and a
`priorityDispositions` array for machine-failure, human-intervention,
split/combine or interface, non-G-code/job-sheet evidence, and learning-feedback
lanes.

Toolpath result reviews are retained CAM/slicer/motion, simulation, dry-run, and
release-package evidence, not certified machine-safety approval. Machine-ready
release remains blocked until toolpath segments, simulations, dry-run gates,
checks, retained artifacts, and human dispositions clear. Stored artifacts
include `toolpath-result`, `toolpath-segments`, `toolpath-simulations`,
`toolpath-checks`, `toolpath-artifacts`, `toolpath-priority-dispositions`, and
`toolpath-learning-observations`. Toolpath observations include
`toolpath-priority:<priority>:<disposition>` so MDP/POMDP/neural workers can
learn when to split parts, combine assemblies, reroute machines, regenerate
motion, change fixtures, or require human review before release.
The `learning.outcomeDraft` uses
`dd.fabrication.toolpath-learning-outcome-draft.v1` with segment, part,
operation, simulation, check, artifact, collision, envelope, clearance, dry-run,
human-intervention, priority-disposition, blocker, reward, and submit-route
hints for direct submission to `POST /fabrication/learning/outcomes`.

## `GET /fabrication/improvements/catalog`

`GET /improvements/catalog` and the gateway-prefixed
`GET /fabrication/improvements/catalog` return the live
`dd.fabrication.instruction-improvement-catalog.v1` repair-draft catalog for
generated and imported instruction streams. The payload exposes improvement
action families, action types, patch operation kinds, planning and
instruction-analysis route aliases, and response surfaces such as
`improvements`, `improvedPrograms`, `improvedPrograms.instructions`,
`improvedPrograms.notes`, `improvedPrograms.machineReady`,
`improvedPrograms.patchManifest`,
`improvedPrograms.patchManifest.operations`,
`improvedPrograms.patchManifest.learningObservations`, `validation.findings`,
`validation.failureBoundaries`, `machineRelease.blockers`, and
`releasePackagePlan.requiredArtifacts`. Catalog entries cover machine-code
modal defaults, finding repair, slicer/additive evidence, advanced additive and
special-process evidence, subtractive/sheet/EDM/turning evidence, assembly,
postprocess, monitoring, and structured text checkpoints. Patch manifests use
`dd.fabrication.instruction-patch-manifest.v1` and emit operation kinds such as
`insert-before-line`, `insert-before-program`,
`insert-before-first-risk-motion`, `insert-after-program`,
`insert-review-checkpoint`, and `review-line`. Improved programs are review
drafts and keep `machineReady=false` until validation, simulation,
controller/postprocessor review, and operator or automation signoff clear.
Instruction-patch observations are emitted for MDP/POMDP/neural workers so
future planning can learn which evidence, defaults, checkpoints, and
split/combine gates reduce failures.

## `GET /fabrication/improvements/preflight/catalog`

`GET /improvements/preflight/catalog` and the gateway-prefixed
`GET /fabrication/improvements/preflight/catalog` return the
`dd.fabrication.instruction-improvement-preflight-catalog.v1` evidence checklist
for patching imported or generated fabrication instructions. The catalog groups
preflight requirements into source-program and finding state, patch-review and
simulation state, and learning plus release feedback state before a repaired
program can move toward release review.

The payload also includes a `patchReviewMatrix` for modal controller-state
repairs, additive printer-state repairs, non-G-code evidence checkpoints, and
split/combine route repairs. Matrix entries name allowed patch operations,
required validation or simulation review, blocked surfaces such as
`controllerPlan.releaseGates`, `interventionMap.splitCombineDecisions`,
`interfaceControlPlan.releaseGates`, and `machineRelease.blockers`, and learning
signals such as `patch-review:additive-state` and `patch-review:split-combine`.

Improvement preflight entries are review requirements, not controller-certified
patches. Improved programs stay `machineReady=false` until source provenance,
patch manifests, validation, simulation or dry-run evidence, controller or
postprocessor compatibility, setup/workholding rechecks, quality/release gates,
and operator or automation signoff clear. Failed checks feed DES, MDP/POMDP, and
neural workers so future plans can add evidence, split jobs, regenerate
instructions, or require human intervention earlier.

## `GET /fabrication/boundaries/catalog`

`GET /boundaries/catalog` and the gateway-prefixed
`GET /fabrication/boundaries/catalog` return the live
`dd.fabrication.boundary-catalog.v1` analyzer boundary catalog before a caller
submits machine code, slicer projects, text job sheets, or generated draft
programs for analysis. The payload exposes boundary families, family counts,
representative detection sources, release evidence requirements, resolution
actions, learning signals, and response surfaces such as
`validation.failureBoundaries`, `boundarySummary`, `resolutionPlan`,
`interventionMap`, `operatorInterventionPlan`, `releaseProbePlan`,
`decompositionPlan`, and `releasePackagePlan`. Machine-ready release remains
blocked while any cataloged machine-failure, human-intervention, split/combine,
automation, postprocess, inspection, profile, or material boundary remains
unresolved, and boundary kinds are emitted as MDP/POMDP/neural learning signals
for regeneration, split/combine, automation-proof, or human-intervention policy
updates.

The payload also includes a `decisionMatrix` that maps boundary families to the
next review lane. Machine-failure boundaries route to instruction improvement,
remediation planning, and simulation/dry-run review; human-intervention and
automation gaps route to intervention, execution, and monitoring planning;
split/combine boundaries route to decomposition, assembly, and interface-control
review; quality/postprocess holds route to quality, postprocess, and release
preview; and resolved or rejected boundaries route to learning-outcome and replay
review before any policy promotion. Matrix entries name blocked surfaces such as
`machineRelease.blockers`, `releaseProbePlan.requiredBeforeRelease`,
`interventionMap.splitCombineDecisions`, `interfaceControlPlan.releaseGates`,
and `learningPolicySnapshot.promotionBlockers`, plus learning signals such as
`boundary-decision:machine-failure`, `boundary-decision:human-intervention`,
`boundary-decision:split-combine`, and `boundary-decision:learning-feedback`.

## `GET /fabrication/boundaries/preflight/catalog`

`GET /boundaries/preflight/catalog` and the gateway-prefixed
`GET /fabrication/boundaries/preflight/catalog` return the live
`dd.fabrication.boundary-preflight-catalog.v1` evidence checklist for analyzer
boundaries before machine-ready release. The catalog groups required evidence
into machine-failure boundary evidence state, human-intervention and automation
gap state, and split-combine/remediation boundary state.

Boundary preflight entries describe evidence required before trusting
machine-failure, human-intervention, automation-gap, and split/combine
decisions; they are not controller-certified safety results. Machine-ready
release remains blocked while boundary source evidence, remediation action,
release probe, intervention owner, split/combine route, or learning feedback is
absent. Failed checks feed DES, MDP/POMDP, and neural workers so future plans can
reroute manufacturing, split parts, regenerate instructions, or require human
intervention earlier.

## `POST /fabrication/boundaries/result`

`POST /boundaries/result` and the gateway-prefixed
`POST /fabrication/boundaries/result` normalize external boundary analyzer
evidence into `dd.fabrication.boundary-analysis-result-review.v1`. Workers can
return findings, machine-failure boundaries, human-intervention gates,
split/combine decisions, retained artifacts, warnings, and metadata for generated
or imported fabrication instructions.

Machine-ready release remains blocked while worker failure, blocking findings,
machine-failure or human-intervention boundaries, split/combine decisions, or
artifact evidence gaps remain. The response stores a boundary-analysis result job
with separate artifacts for findings, boundaries, split/combine decisions,
retained analyzer artifacts, and learning observations. It also drafts
`dd.fabrication.boundary-analysis-learning-outcome-draft.v1` feedback for
MDP/POMDP/neural learners so future plans can split work earlier, combine parts
deliberately, choose safer machine routes, or insert operator checkpoints before
the failure boundary.

## `GET /fabrication/remediation/catalog`

`GET /remediation/catalog` and the gateway-prefixed
`GET /fabrication/remediation/catalog` return the live
`dd.fabrication.boundary-remediation-catalog.v1` remediation-lane catalog for
machine-failure, human-intervention, automation, split/combine, postprocess,
inspection, profile, and material boundaries found in generated or imported
fabrication instructions. The payload maps each boundary kind to detection
surfaces, required release evidence, remediation actions, route handoffs,
response surfaces such as `resolutionPlan.steps`, `interventionMap`,
`operatorInterventionPlan`, `improvedPrograms.patchManifest`,
`decompositionPlan`, `interfaceControlPlan`, `machineRelease.blockers`, and
`releasePackagePlan.requiredArtifacts`, plus MDP/POMDP/neural learning signals.

Remediation catalog entries are ranked review lanes, not certified controller
repairs. `machineReady=false` remains mandatory until remediation evidence,
validation, simulation or dry-run evidence, controller/postprocessor review,
split/combine review, and operator or automation signoff clear. Remediation
signals help learning workers decide when future jobs should choose safer
machines, split or combine parts differently, regenerate instructions, or insert
human checkpoints before hardware execution.

## `POST /fabrication/remediation/plan`

`POST /remediation/plan` and the gateway-prefixed
`POST /fabrication/remediation/plan` accept the same instruction-analysis payload
as `POST /fabrication/instructions/analyze` and return
`dd.fabrication.boundary-remediation-planning.v1`. The endpoint retains the full
analysis artifacts, then focuses the response on `remediationPlan.actions`,
`validation.failureBoundaries`, `resolutionPlan.steps`,
`operatorInterventionPlan`, `machineRelease.blockers`,
`improvedPrograms.patchManifest`, and route handoffs to validation, instruction
improvement, simulation or dry-run, decomposition/assembly, execution/monitoring,
and release preview.

Remediation plans are review and worker-handoff contracts, not
controller-certified corrections. They keep `machineReady=false` until the caller
retains updated instruction evidence, validation and simulation results,
controller/postprocessor review, split/combine interface evidence, and operator
or automation signoff. Each remediation action also emits MDP/POMDP/neural
learning signals such as boundary kind, resolution action, split/combine review,
and operator-intervention keys so future plans can choose safer machines, split
or combine earlier, or insert human checkpoints before hardware execution.

## `POST /fabrication/remediation/result`

`POST /remediation/result` and the gateway-prefixed
`POST /fabrication/remediation/result` accept external remediation-worker review
results after a boundary remediation plan has been attempted. The
`dd.fabrication.boundary-remediation-result-review.v1` response retains
`boundaryRemediationResult.actions`, retained remediation artifacts, validation
evidence, simulation or dry-run evidence, release blockers, and learning
observations for downstream MDP/POMDP/neural outcome memory. It also includes a
`dd.fabrication.boundary-remediation-learning-outcome-draft.v1` payload with
remediator, action, boundary, blocker, artifact, validation/simulation evidence,
human-signoff, reward, and submit-route hints for
`POST /fabrication/learning/outcomes`.

The result route is still a review gate, not a machine certification. Even when a
worker reports success, `machineReady=false` and `releaseBlocked=true` remain in
force until every action has evidence, artifacts have URI/checksum/evidence,
validation and simulation proof are attached, and release package, controller,
setup, monitoring, and operator or automation signoff gates clear.

## `GET /fabrication/decomposition/catalog`

`GET /decomposition/catalog` and the gateway-prefixed
`GET /fabrication/decomposition/catalog` return the live
`dd.fabrication.decomposition-catalog.v1` split/combine and interface-control
catalog before a caller submits generated or imported work. The payload exposes
decomposition target families, family counts, target kinds, representative route
machine kinds, required child-geometry and per-route evidence, interface-control
fit modes, release gates, planning and instruction-analysis route aliases, and
response surfaces such as `hybridMakePlan.splitCombineDecisions`,
`decompositionPlan.targets`, `decompositionPlan.routeContracts`,
`decompositionPlan.recompositionInterfaces`, `decompositionPlan.releaseGates`,
`interfaceControlPlan.controls`, `interfaceControlPlan.decisionLinks`, and
`releasePackagePlan.packages`. Catalog entries are draft decomposition and
interface-control contracts, not certified assembly release; machine-ready
release remains blocked until child geometry, per-route machine code, datum
transfer, interface metrology, recomposition, and operator or automation
evidence are retained. The catalog also exposes learning surfaces such as
`decompositionPlan.learningObservations`,
`interfaceControlPlan.learningObservations`, `mdp-request` decomposition and
interface-control artifacts, and retained `learning.outcomes` so MDP/POMDP/neural
workers can compare single-piece, split-route, and recomposed outcomes.

## `POST /fabrication/decomposition/plan`

`POST /decomposition/plan` and the gateway-prefixed
`POST /fabrication/decomposition/plan` run the planner and return a compact
`dd.fabrication.decomposition-planning.v1` split/combine package for generated or
imported fabrication requests. The response keeps the full plan stored and
published through the normal plan-result path while focusing the caller on
`decompositionPlan.targets`, `decompositionPlan.routeContracts`,
`decompositionPlan.recompositionInterfaces`, `interfaceControlPlan.controls`,
`hybridMakePlan.partRoutes`, `hybridMakePlan.joinOperations`,
`hybridMakePlan.splitCombineDecisions`, `assembly.assemblyGraph`,
`releasePackagePlan.packages`, and `machineRelease.blockers`.

The endpoint is a draft decomposition lane, not certified CAD/CAM/slicer,
assembly, or controller output. Machine-ready release remains blocked until
child geometry, per-route machine code, datum transfer, interface metrology,
recomposition, release package, and operator or automation evidence are retained.
It stores artifacts such as `decomposition-plan`, `interface-control-plan`,
`hybrid-make-plan`, `assembly-plan`, `release-package-plan`, and `mdp-request`
so MDP/POMDP/neural workers can learn when single-piece, split-route, or
recomposed fabrication succeeds.

## `POST /fabrication/decomposition/result`

`POST /decomposition/result` and the gateway-prefixed
`POST /fabrication/decomposition/result` normalize external split/combine,
decomposition, and hybrid-route worker results into
`dd.fabrication.decomposition-result-review.v1`. The route accepts target
reviews, route reviews, interface checks, split/combine decisions, artifacts,
worker or decomposer identity, warnings, and metadata after a worker tries to
prove whether one body must be split, multiple parts can be recomposed, or a
hybrid route is release-ready.

The response exposes `decompositionResult`, `decompositionResultJobId`,
`generatedAtMs`, `releaseBlocked`, target, route, interface, split/combine, and
human-intervention blocker counts, missing artifact evidence, and follow-up
assembly, release, and learning routes. The learning section also emits a
`dd.fabrication.decomposition-learning-outcome-draft.v1` `outcomeDraft` with
target, route, interface, split/combine, artifact, blocker, redesign, and
human-intervention hints ready for `POST /fabrication/learning/outcomes`.
Successful reviews are retained under `decompositionResultJobId`; `/jobs/:job_id` and
`/jobs/:job_id/artifacts/:artifact_id` can inspect `decomposition-result`,
`decomposition-targets`, `decomposition-route-reviews`,
`decomposition-interfaces`, `decomposition-split-combine-decisions`,
`decomposition-artifacts`, and `decomposition-learning-observations`. Machine-ready
release stays blocked until split/combine targets, routes, interfaces,
recomposition decisions, and artifacts have evidence. Result observations
include `decomposition-target:*`, `decomposition-route:*`,
`decomposition-interface:*`, `decomposition-decision:*`, and
`decomposition-artifact:*`.

## `GET /fabrication/assembly/catalog`

`GET /assembly/catalog` and the gateway-prefixed
`GET /fabrication/assembly/catalog` return the live
`dd.fabrication.assembly-catalog.v1` hybrid assembly, recomposition, and joining
catalog before a caller treats printed, milled, turned, sheet-cut, EDM, or
special-process child routes as one released object. The payload exposes evidence
contracts for `assembly.assemblyGraph`, `hybridMakePlan.joinOperations`,
`hybridMakePlan.splitCombineDecisions`, `interfaceControlPlan.controls`,
`qualityPlan.measurementTargets`, generated `assembly-cell-job` or
metal-joining travelers, and `releasePackagePlan.packages`. It names worker
families for assembly planning, interface-control review, metrology, robotic-cell
review, metal-joining review, release packaging, and learning-outcome feedback.
Catalog entries are worker-lane evidence contracts, not certified assembly or
robot-cell release; machine-ready release remains blocked until child route
packages, datum transfer, dry-fit or metrology, join recipe evidence, interlock
or operator signoff, final inspection, and outcome feedback are retained.
Assembly, interface, quality, release, and outcome observations feed
MDP/POMDP/neural workers so future plans can learn when to split, combine,
recompose, or keep a part single-piece.

## `GET /fabrication/assembly/preflight/catalog`

`GET /assembly/preflight/catalog` and the gateway-prefixed
`GET /fabrication/assembly/preflight/catalog` return the live
`dd.fabrication.assembly-preflight-catalog.v1` release checklist before printed,
milled, turned, sheet-cut, EDM, robotic, or special-process child routes are
combined into one released object. The catalog groups required evidence into
child-route package and interface state, join-recipe fixture and process state,
and final-fit quality release plus learning state.

Preflight entries describe required release evidence, not certified assembly,
robot-cell, or joining instructions. Machine-ready release remains blocked while
child route packages, interface controls, join recipes, fixtures, final fit,
quality, disposition, or operator/automation signoff evidence is absent. Failed
checks feed DES, MDP/POMDP, and neural workers so future planners can split,
combine, recompose, redesign, reroute, or require human intervention earlier.

## `POST /fabrication/assembly/plan`

`POST /assembly/plan` and the gateway-prefixed
`POST /fabrication/assembly/plan` run the planner and return a compact
`dd.fabrication.assembly-planning.v1` recomposition package for printed, milled,
turned, sheet-cut, EDM, robotic, or special-process child routes. The response
keeps the full plan stored and published through the normal plan-result path
while focusing the caller on `assembly.assemblyGraph`,
`assembly.assemblyGraph.interfaces`, `assembly.assemblyGraph.sequence`,
`hybridMakePlan.joinOperations`, `hybridMakePlan.splitCombineDecisions`,
`interfaceControlPlan.controls`, `qualityPlan.inspectionPoints`,
`releasePackagePlan.packages`, `releasePackagePlan.releaseGates`, and
`machineRelease.blockers`.

The endpoint is a draft assembly lane, not certified assembly, robot-cell,
inspection, or controller output. Machine-ready release remains blocked until
child route packages, interface controls, dry-fit or metrology, join recipe,
inspection, release package, and operator or automation signoff are retained. It
stores artifacts such as `assembly-plan`, `hybrid-make-plan`,
`interface-control-plan`, `quality-plan`, `release-package-plan`, and
`mdp-request` so MDP/POMDP/neural workers can learn which recomposition and join
strategies complete without hidden human intervention.

## `POST /fabrication/assembly/result`

`POST /assembly/result` and the gateway-prefixed
`POST /fabrication/assembly/result` normalize external assembly planning worker
results from `dd.remote.fabrication.assembly.planning.results` into a compact
`dd.fabrication.assembly-planning-result-review.v1` package. The route accepts
part routes, join operations, split/combine decisions, interface checks, retained
artifacts, worker identity, warnings, and metadata after an assembly or
recomposition planner attempts to turn child fabrication routes into one
machine-release candidate.

The response exposes `assemblyPlanningResult`, `assemblyResultJobId`,
`generatedAtMs`, `releaseBlocked`, `routeBlockerCount`, `joinBlockerCount`,
`splitCombineBlockerCount`, `interfaceBlockerCount`,
`missingArtifactEvidenceCount`, and the assembly-planning request/queue/result
subjects. It also includes a
`dd.fabrication.assembly-planning-learning-outcome-draft.v1` payload with route,
join, split/combine, interface-check, artifact, human-intervention, reward, and
submit-route hints for `POST /fabrication/learning/outcomes`. Successful reviews are retained in the bounded job ledger under
`assemblyResultJobId`; `/jobs/:job_id` and
`/jobs/:job_id/artifacts/:artifact_id` can inspect
`assembly-planning-result`, `assembly-part-routes`, `assembly-join-operations`,
`assembly-split-combine-decisions`, `assembly-interface-checks`,
`assembly-artifacts`, and `assembly-learning-observations`. Machine-ready
release remains blocked until routes are accepted, joins and recomposition
decisions clear or become explicit human-intervention gates, interface checks
pass datum/fit/tolerance review, and artifacts carry URI/checksum/evidence
labels. Result observations include `assembly-part-route:*`,
`assembly-join:*`, `assembly-split-combine:*`, `assembly-interface-check:*`,
and `assembly-artifact:*` signals so MDP/POMDP/neural workers can learn which
split/combine and join boundaries cleared or blocked recomposed hardware.

## `POST /fabrication/interfaces/result`

`POST /interfaces/result` and the gateway-prefixed
`POST /fabrication/interfaces/result` normalize external interface-control
worker evidence into `dd.fabrication.interface-result-review.v1`. The route
accepts datum-transfer and fit checks, join evidence for adhesive, plastic weld,
press-fit, fastener, seal, bearing, or metal-joining interfaces,
split/combine decisions, retained artifacts, worker identity, warnings, and
metadata after a decomposition or assembly worker tries to prove that printed,
milled, turned, molded, or special-process parts can be recomposed safely.

The response exposes `interfaceResult`, `interfaceResultJobId`, `generatedAtMs`,
`releaseBlocked`, `interfaceBlockerCount`, `joinBlockerCount`,
`splitCombineBlockerCount`, `humanInterventionRequired`, and
`missingArtifactEvidenceCount`. It also includes a
`dd.fabrication.interface-learning-outcome-draft.v1` payload with interface,
join, split/combine, artifact, human-intervention, reward, and submit-route
hints for `POST /fabrication/learning/outcomes`. Successful reviews are retained
in the bounded job ledger under `interfaceResultJobId`; `/jobs/:job_id` and
`/jobs/:job_id/artifacts/:artifact_id` can inspect `interface-result`,
`interface-checks`, `interface-join-evidence`,
`interface-split-combine-decisions`, `interface-artifacts`, and
`interface-learning-observations`. Machine-ready release remains blocked until
datum transfer, fit, join recipe, operator or automation proof, split/combine
decision, and artifact evidence clear. Result observations include
`interface-kind:*`, `interface-join:*`, `interface-split-combine:*`, and
`interface-artifact:*` signals so MDP/POMDP/neural workers can learn which
interface boundaries require redesign, rework, alternate joining, or human
intervention before recomposition.

## `GET /fabrication/release/catalog`

`GET /release/catalog` and the gateway-prefixed
`GET /fabrication/release/catalog` return the live
`dd.fabrication.release-catalog.v1` machine-ready release catalog before a
caller treats generated programs, improved instructions, or assembly travelers as
shop-floor release packets. The payload exposes release package kinds, package
states, gate types, blocker sources, required artifacts, planning and
instruction-analysis route aliases, and response surfaces such as
`machineRelease.status`, `machineRelease.blockers`, `machineRelease.checklist`,
`releasePackagePlan.packages`, `releasePackagePlan.releaseGates`,
`releasePackagePlan.requiredArtifacts`, `releasePackagePlan.learningObservations`,
`controllerPlan.releaseGates`, `postprocessPlan.blockers`,
`simulation.riskProfile`, `decompositionPlan.releaseGates`, and
`interfaceControlPlan.releaseGates`. Catalog entries are machine-ready evidence
contracts, not certified equipment safety; machine-ready release remains blocked
until validation findings, failure boundaries, release probes,
controller/postprocessor checks, simulation or dry-run evidence, split/combine
interface gates, and operator or automation signoff clear. Release package
observations are emitted for MDP/POMDP/neural workers so future planning can
learn which evidence cleared or blocked printed, milled, turned, sheet-cut, EDM,
and recomposed routes.

## `GET /fabrication/release/preflight/catalog`

`GET /release/preflight/catalog` and the gateway-prefixed
`GET /fabrication/release/preflight/catalog` return the
`dd.fabrication.release-preflight-catalog.v1` evidence checklist for the final
machine-ready handoff. The catalog groups release blockers across
manifest/artifact/checksum state, machine/controller/simulation/process state, and
quality/disposition/signoff/learning state. It names required response surfaces
such as `releasePackagePlan.requiredArtifacts`,
`releaseReadinessResult.manifestArtifacts`, `machineRelease.blockers`,
`validation.failureBoundaries`, `qualityResult.findings`,
`dispositionResult.decisions`, and `learningOutcome.observations`.

Release preflight entries are evidence requirements, not equipment safety
certifications. Machine-ready release remains blocked while artifact provenance,
controller/postprocessor compatibility, dry-run or simulation evidence,
workholding/setup, monitoring, split/combine traceability, quality disposition,
operator or automation signoff, or DES/MDP/POMDP/neural learning feedback is
missing.

## `POST /fabrication/release/preview`

`POST /release/preview` and the gateway-prefixed
`POST /fabrication/release/preview` accept the same request body as
`POST /fabrication/plan`, apply the current bounded learning-policy memory, run
the planner, and return a compact `dd.fabrication.release-preview.v1` advisory
envelope. The response focuses on `machineRelease.status`,
`machineRelease.blockers`, `machineRelease.checklist`,
`releasePackagePlan.packages`, `releasePackagePlan.releaseGates`,
`releasePackagePlan.requiredArtifacts`, `executionPlan.stopPoints`,
`postprocessPlan.blockers`, `controllerPlan.compatibilityTargets`,
`simulation.riskProfile`, operator intervention requirements,
split/combine decisions, release-probe learning surfaces, and
`manufacturingHandoff`. Release previews do not retain full plan jobs; they are
retained as compact `release-preview` jobs with machine-release, package, execution, simulation,
operator-intervention, and policy-summary artifacts for later release-bundle inspection. They do not publish controller code or certify machine-ready
artifacts; they keep `machineReady=false` while machine-release, controller, postprocess, simulation,
setup, intervention, split/combine, schedule, or package gates remain blocked.

## `POST /fabrication/release/result`

`POST /release/result` and the gateway-prefixed
`POST /fabrication/release/result` normalize external final release-readiness
worker results from `dd.remote.fabrication.release.readiness.results` into a
compact `dd.fabrication.release-readiness-result-review.v1` package. The route
accepts release decisions, retained manifest artifacts, blockers, human
intervention gates, worker identity, warnings, and metadata after generated,
imported, simulated, reviewed, split, combined, or recomposed work reaches the
machine-release gate.

The response exposes `releaseReadinessResult`, `releaseResultJobId`,
`generatedAtMs`, `releaseBlocked`, `blockedDecisionCount`, `blockerCount`,
`pendingHumanInterventionCount`, `missingManifestEvidenceCount`, and the
release-readiness request/queue/result subjects plus a
`dd.fabrication.release-readiness-learning-outcome-draft.v1` payload that callers
can send to `POST /fabrication/learning/outcomes`. Successful reviews are
retained in the bounded job ledger under `releaseResultJobId`; `/jobs/:job_id`
and `/jobs/:job_id/artifacts/:artifact_id` can inspect
`release-readiness-result`, `release-readiness-decisions`,
`release-readiness-manifest-artifacts`, `release-readiness-blockers`,
`release-readiness-human-interventions`, and
`release-readiness-learning-observations`. Machine-ready release remains blocked
until every decision is machine-ready or explicitly cleared, manifest artifacts
carry URI/checksum/evidence labels, blockers are cleared or converted to release
conditions, and required operator, automation, split/combine, and signoff gates
are complete. Result observations include `release-readiness-decision:*`,
`release-readiness-blocker:*`, `release-readiness-intervention:*`, and
`release-readiness-artifact:*` signals so MDP/POMDP/neural workers can learn
which final gates cleared or blocked hardware execution.

## `POST /fabrication/execution/plan`

`GET /execution/preflight/catalog` and the gateway-prefixed
`GET /fabrication/execution/preflight/catalog` expose the
`dd.fabrication.execution-preflight-catalog.v1` run-readiness contract before a
planner request is submitted. The catalog groups required evidence for
program-run/machine state, stop-point/human-intervention/automation state, and
monitoring/recovery/release state. It names the `executionPlan`,
`operatorInterventionPlan`, `interventionMap`, `machineSchedule`,
`monitoringPlan`, `machineRelease`, `releasePackagePlan`, and learning surfaces
that must be reviewable before generated or imported printer, mill, lathe,
router, sheet-cutting, assembly, or special-process instructions are considered
ready to run.

`POST /execution/plan` and the gateway-prefixed
`POST /fabrication/execution/plan` accept the same request body as
`POST /fabrication/plan`, apply bounded learning-policy memory, run the planner,
store and publish the full plan result, and return a compact
`dd.fabrication.execution-planning.v1` preflight package. The response focuses on
`executionPlan.programRuns`, `executionPlan.checkpoints`,
`executionPlan.stopPoints`, `operatorInterventionPlan.requiredOperatorActions`,
`operatorInterventionPlan.evidenceGates`,
`operatorInterventionPlan.automationCandidates`,
`operatorInterventionPlan.splitCombineReviews`, `interventionMap`,
`machineSchedule.operations`, `machineSchedule.dependencyHolds`,
`monitoringPlan.monitorPoints`, `monitoringPlan.recoveryActions`,
`machineRelease.blockers`, `simulation.riskProfile`, and
`learning.releaseProbePlan`.

Execution plans are conservative preflight packages, not certified controller
safety, unattended-run authorization, or shop-floor restart instructions.
Machine-ready release remains blocked while stop points, required operator
actions, unresolved evidence gates, dependency holds, monitoring recovery gaps,
or release blockers remain open. Stored artifacts include `execution-plan`,
`operator-intervention-plan`, `machine-schedule`, `monitoring-plan`,
`machine-release`, `simulation-report`, and `mdp-request` so
MDP/POMDP/neural workers can learn when to add automation, split jobs,
regenerate instructions, or keep human checkpoints.

## `POST /fabrication/execution/result`

`POST /execution/result` and the gateway-prefixed
`POST /fabrication/execution/result` normalize execution telemetry from
`dd.remote.fabrication.execution.telemetry.results` into a compact
`dd.fabrication.execution-result-review.v1` package. The endpoint accepts
observed printer, mill, lathe, router, sheet-cutting, assembly, inspection, or
postprocess run state with run segments, machine stops, operator interventions,
split/combine decisions, retained telemetry artifacts, metrics, warnings, and
worker metadata.

The response exposes `executionResult`, `executionResultJobId`, `generatedAtMs`,
`executionBlocked`, `blockingMachineStopCount`,
`restartBlockingOperatorInterventionCount`, `splitCombineBlockerCount`,
`missingArtifactEvidenceCount`, and the execution-telemetry
request/queue/result subjects. It also includes a
`dd.fabrication.execution-learning-outcome-draft.v1` payload with run, machine,
state, stop, operator-action, split/combine, artifact, reward, and submit-route
hints for `POST /fabrication/learning/outcomes`. Successful reviews are retained in the bounded
job ledger under `executionResultJobId`; `/jobs/:job_id` and
`/jobs/:job_id/artifacts/:artifact_id` can inspect `execution-result`,
`execution-run-segments`, `execution-machine-stops`,
`execution-operator-interventions`, `execution-split-combine-decisions`,
`execution-artifacts`, and `execution-learning-observations`. Repeat execution
or machine-ready release remains blocked until machine stops, unresolved
operator interventions, split/combine or redesign decisions, and telemetry
artifacts clear with retained evidence. Execution observations include
`execution-stop:*`, `execution-stop-kind:*`,
`execution-recommended-action:*`, `execution-operator-action:*`,
`execution-split-combine:*`, and `execution-artifact:*` signals so
MDP/POMDP/neural workers can learn which real runs succeeded, failed, required
human intervention, or forced separating/combining parts.

## `GET /fabrication/strategy/catalog`

`GET /strategy/catalog` and the gateway-prefixed
`GET /fabrication/strategy/catalog` return the live
`dd.fabrication.strategy-catalog.v1` decision-strategy catalog before callers
treat scored route choices or learned preferences as authoritative process
selection. The payload exposes advisory contracts for `strategyCandidates`,
`strategyCandidates.score`, `strategyCandidates.rationale`,
`hybridMakePlan.partRoutes`, `hybridMakePlan.joinOperations`,
`hybridMakePlan.splitCombineDecisions`, `learning.enginePolicy`,
`pomdpBeliefState.hiddenStates`, `releaseProbePlan.probes`,
`neuralPolicy.engineInference`, `neuralTrainingCorpus.inferenceCandidates`, and
`interventionSignals`. Retained artifacts include `mdp-request` strategy
candidates, hybrid make plans, DES MDP/POMDP specs and solutions, POMDP belief
state, release probe plan, neural training corpus, and the `parametric-design`
hybrid make plan embedding. Strategy catalog entries are advisory decision,
learning, and evidence-handoff contracts, not certified manufacturing strategy
approval; learned preferences can bias open-ended planning only when caller
preferences are absent, and machine-ready release remains blocked until
validation, setup, simulation, quality, intervention, postprocess, schedule, and
release blockers clear.

## `GET /fabrication/hybrid/catalog`

`GET /hybrid/catalog` and the gateway-prefixed
`GET /fabrication/hybrid/catalog` return the live
`dd.fabrication.hybrid-catalog.v1` split/combine discovery view for parts made
across printed, milled, turned, cut, inspected, postprocessed, and assembled
routes. The payload consolidates the relevant manufacturing method families,
printer/subtractive/cell machine catalogs, decomposition and assembly planning
routes, strategy recommendation, result-review routes, response surfaces,
artifact surfaces, and release policy for `hybridMakePlan`,
`decompositionPlan`, `interfaceControlPlan`, `assembly.assemblyGraph`,
`releasePackagePlan`, POMDP belief state, and neural training corpus handoffs.

Catalog entries are advisory split/combine and recomposition workflows, not
certified manufacturing release. Hybrid routes remain blocked until interface,
datum, tolerance, workholding, join, simulation, quality, and operator or
automation evidence clear; retained observations feed DES, MDP/POMDP, and neural
policy workers so future plans can choose one-piece, split-route, recomposed, or
human-intervention paths earlier.

## `GET /fabrication/methods/catalog`

`GET /methods/catalog` and the gateway-prefixed
`GET /fabrication/methods/catalog` return the live
`dd.fabrication.manufacturing-method-catalog.v1` process-family discovery
catalog before clients ask the planner to choose, split, combine, or learn a
manufacturing route. The payload groups additive printing, subtractive
milling/routing, turning and mill-turn, sheet cutting and EDM, hybrid
split/combine assembly, finishing/postprocess/quality, inspection/calibration,
and special-process methods with representative machine kinds, CAD/design
inputs, instruction kinds, release blockers, response surfaces, artifact
surfaces, and learning signals.

The catalog is advisory and not certified live machine availability: it tells
clients which method families can feed `strategyCandidates.methods`,
`hybridMakePlan.partRoutes`,
`decompositionPlan.parts`, `interfaceControlPlan.interfaces`,
`machineSelection.selectedMachineKind`, `machineSchedule.operations`,
`materialPlan.routeRequirements`, `postprocessPlan.controllerTargets`,
`qualityPlan.requirements`, `machineRelease.blockers`, and learning reward
terms. Learned method preferences can bias print/mill/turn/cut/inspect/join
choices only while deterministic validation, setup, controller, simulation,
quality, intervention, postprocess, and release gates remain authoritative.

## `POST /fabrication/strategy/recommend`

`POST /strategy/recommend` and the gateway-prefixed
`POST /fabrication/strategy/recommend` accept the same request body as
`POST /fabrication/plan`, apply the current bounded learning-policy memory, run
the planner, and return a compact
`dd.fabrication.strategy-recommendation.v1` advisory envelope. The response
focuses on `strategyCandidates`, the top scored candidate,
`hybridMakePlan.selectedStrategy`, part routes, join operations,
split/combine decisions, policy-memory counts, `policySummary.successRate`,
`policySummary.failureRate`, `learningOutcomeQuality`, DES-backed
`learning.enginePolicy`, POMDP belief/probe surfaces, neural-policy inference,
intervention signals, and `machineRelease.blockers`. `learningOutcomeQuality`
summarizes learned success/failure rates, reward average, remediation-risk count,
and a `releasePolicy` such as `positive-learned-route-evidence` or
`review-learned-route-quality-before-release` so clients can require route review
when prior failures or negative rewards are present. Recommendation responses
do not retain full plan jobs, publish generated controller code, or certify
machine-ready release; they keep `machineReady=false` while validation, setup,
simulation, quality, postprocess, schedule, intervention, or release blockers
remain.

## `POST /fabrication/strategy/result`

`POST /strategy/result` and the gateway-prefixed
`POST /fabrication/strategy/result` let external strategy optimizers report
whether a recommended print/mill/turn/cut/inspect/split/combine route is actually
feasible, releasable, and useful for learning. The
`dd.fabrication.strategy-result-review.v1` envelope validates route reviews,
split/combine decisions, MDP/POMDP/neural learning updates, retained artifacts,
warning text, and optimizer metadata, then stores a `strategy-result` job with
`strategy-route-reviews`, `strategy-split-combine-decisions`,
`strategy-learning-updates`, `strategy-artifacts`, and
`strategy-learning-observations` artifacts. The learning section emits a
`dd.fabrication.strategy-learning-outcome-draft.v1` `outcomeDraft` with
optimizer, selected-strategy, method, machine-kind, route, split/combine,
policy-update, artifact, blocker, redesign, and human-intervention hints ready
for `POST /fabrication/learning/outcomes`.

Machine-ready release remains blocked when the optimizer failed, no route review
was supplied, a route is infeasible, split/combine or redesign decisions are
unaccepted, learning updates still require review, or retained artifacts lack a
URI, checksum, and evidence. Observations such as `strategy-route:*`,
`strategy-route-machine-kind:*`, `strategy-split-combine:*`,
`strategy-learning-update:*`, and `strategy:human-intervention-required` feed the
bounded learning memory so later planners can avoid blocked hybrid routes or ask
for redesign and operator intervention earlier.

## `GET /fabrication/schedule/catalog`

`GET /schedule/catalog` and the gateway-prefixed
`GET /fabrication/schedule/catalog` return the live
`dd.fabrication.schedule-catalog.v1` production batching, machine-lane
scheduling, dependency-hold, and DES queue-model catalog before callers treat a
plan as shop-floor dispatch. The payload exposes contracts for
`productionPlan.batches`, `productionPlan.totalEstimatedMachineMinutes`,
`machineSchedule.machineLanes`,
`machineSchedule.machineLanes.utilizationRatio`,
`machineSchedule.operations`, `machineSchedule.dependencyHolds`, and
`desScheduleModel.laneModels`. Retained artifacts include `production-plan`,
`machine-schedule`, `des-schedule-model`, their `parametric-design` embeddings,
and matching `mdp-request` artifact payloads. Schedule catalog entries are
deterministic production and DES handoff contracts, not certified MES dispatch
or controller output; machine-ready release remains blocked while batches,
scheduled operations, dependency holds, release blockers, postprocess targets,
setup evidence, or operator/automation assignments are unresolved. Schedule and
DES observations are retained for MDP/POMDP/neural workers so future planning
can learn which machines, batch sizes, split/combine routes, and setup sequences
reduce blocked starts or human intervention.

## `POST /fabrication/schedule/result`

`POST /schedule/result` and the gateway-prefixed
`POST /fabrication/schedule/result` accept scheduler, DES, MDP/POMDP optimizer,
or dispatch worker results for production batches, machine lanes, operation
windows, dependency holds, and queue models. They normalize those records into
`dd.fabrication.schedule-result-review.v1`, retain a bounded review job, and
expose a `dd.fabrication.schedule-learning-outcome-draft.v1` payload that
callers can send to `POST /fabrication/learning/outcomes` plus release blocker
counts for blocked lanes, overcapacity, invalid operation windows, unresolved
holds, unstable DES models, missing artifact evidence, and human interventions.

Schedule result reviews are retained production and DES evidence, not certified
MES dispatch authorization. Machine-ready release remains blocked until machine
lanes, scheduled operations, dependency holds, DES queue models, artifacts, and
human dispositions clear. Stored artifacts include `schedule-result`,
`schedule-lanes`, `schedule-operations`, `schedule-holds`,
`schedule-des-models`, `schedule-artifacts`, and
`schedule-learning-observations` so DES/MDP/POMDP/neural workers can learn when
to change batch sizes, reroute machines, split parts, combine assemblies,
resequence setup work, or require a human assignment before release.

## `GET /fabrication/simulation/catalog`

`GET /simulation/catalog` and the gateway-prefixed
`GET /fabrication/simulation/catalog` return the live
`dd.fabrication.simulation-catalog.v1` dry-run and simulation-risk catalog before
callers treat generated or imported programs as machine-ready. The payload
exposes toolpath-envelope, arc-sweep, rapid-clearance, process-start, and
rotary/index review risk contracts; dry-run evidence contracts; planning and
instruction-analysis route aliases; and response surfaces such as
`simulation.programs`, `simulation.programs.axisExtents`,
`simulation.programs.safeClearanceObserved`,
`simulation.programs.spindleOrHeatupObserved`, `simulation.riskProfile`,
`simulation.riskProfile.programRisks`,
`simulation.riskProfile.learningObservations`, `simulation.findings`,
`simulation.failureBoundaries`, `validation.failureBoundaries`,
`machineRelease.blockers`, `executionPlan.stopPoints`, and
`releaseProbePlan.probes`. Catalog entries are dry-run and risk evidence
contracts, not certified machine safety; machine-ready release remains blocked
while simulation risk is blocked, envelope or clearance boundaries remain open,
process-start proof is missing, or required dry-run artifacts such as
`simulation-report`, `analysis-simulation-report`,
`rotary-clearance-simulation-report`, or
`robot-path-or-fixture-simulation-report` are absent. Simulation-risk
observations are emitted for MDP/POMDP/neural workers so future planning can
learn when to reroute, split parts, add clearance, or require operator review.

## `GET /fabrication/simulation/preflight/catalog`

`GET /simulation/preflight/catalog` and the gateway-prefixed
`GET /fabrication/simulation/preflight/catalog` return the live
`dd.fabrication.simulation-preflight-catalog.v1` evidence checklist for dry-run
or simulation readiness before risk results can influence release. The catalog
groups preflight requirements into machine envelope/fixture/datum state,
controller/process/program state, and dry-run/release/learning state.

Simulation preflight entries are release gates, not certified machine safety.
Machine-ready release remains blocked while machine envelope, fixture, datum,
controller, process-start, retained dry-run artifacts, risk findings, signoff,
release-package links, or learning evidence is missing. Failed checks feed DES,
MDP/POMDP, and neural workers so future plans can reroute, split or combine
work, add clearance, regenerate programs, or require human intervention earlier.

## `POST /fabrication/simulation/run`

`POST /simulation/run` and the gateway-prefixed
`POST /fabrication/simulation/run` run the planner and return a compact
`dd.fabrication.simulation-run.v1` dry-run and risk package for generated or
imported programs. The response keeps the full plan stored and published
through the normal plan-result path while focusing callers on
`simulation.programs`, `simulation.programs.axisExtents`,
`simulation.riskProfile`, `simulation.riskProfile.programRisks`,
`simulation.findings`, `simulation.failureBoundaries`,
`machineRelease.blockers`, `executionPlan.stopPoints`,
`postprocessPlan.targets`, `releasePackagePlan.packages`, and
`learning.releaseProbePlan`.

The endpoint is a draft dry-run and risk lane, not certified machine safety,
controller proof, or shop-floor authorization. Machine-ready release remains
blocked while simulation risk is blocked or review-required, envelope or
clearance boundaries remain open, process-start proof is missing, or required
dry-run artifacts are absent. It stores artifacts such as `simulation-report`,
`execution-plan`, `postprocess-plan`, `machine-release`,
`release-package-plan`, and `mdp-request` so MDP/POMDP/neural workers can learn
when to reroute, split parts, add clearance, regenerate programs, or require
operator review.

## `POST /fabrication/simulation/result`

`POST /simulation/result` and the gateway-prefixed
`POST /fabrication/simulation/result` normalize external simulation, dry-run,
collision, clearance, thermal, fixture, and material-flow worker results from
`dd.remote.fabrication.instructions.simulation.results` back into a compact
`dd.fabrication.instruction-simulation-result-review.v1` package. The endpoint
accepts envelope checks, simulation findings, failure boundaries, retained
artifact evidence, worker and simulator identity, warnings, and metadata for
generated or imported printer, mill, router, lathe, setup-sheet, and operator
instruction streams.

The response exposes `instructionSimulationResult`, `releaseUpdate`,
`simulationResultJobId`, `generatedAtMs`, `releaseBlocked`,
`blockedEnvelopeCheckCount`, `blockingFindingCount`, `failureBoundaryCount`,
`humanInterventionBoundaryCount`, `missingArtifactEvidenceCount`, and the
request/queue/result subjects for the instruction-simulation worker lane. It
also includes a `priorityDispositions` array for machine-failure,
human-intervention, split/combine or interface, non-G-code/job-sheet evidence,
and learning-feedback lanes.
It also includes a
`dd.fabrication.instruction-simulation-learning-outcome-draft.v1` payload with
simulator, check, finding, boundary, recommended-action, artifact,
priority-disposition, reward, and submit-route hints for
`POST /fabrication/learning/outcomes`.
Successful result reviews are also retained in the bounded job ledger under
`simulationResultJobId`, where `/jobs/:job_id` and
`/jobs/:job_id/artifacts/:artifact_id` can inspect `instruction-simulation-result`,
`instruction-simulation-envelope-checks`, `instruction-simulation-findings`,
`instruction-simulation-failure-boundaries`, `instruction-simulation-artifacts`,
`instruction-simulation-priority-dispositions`, and
`instruction-simulation-learning-observations`. Machine-ready release remains
blocked until simulation checks pass, failure boundaries are resolved or
accepted, dry-run artifacts are retained with URI/checksum/evidence labels, and
required operator, split/combine, regeneration, or reroute decisions are
attached. Result observations include `instruction-simulation-check:*`,
`instruction-simulation-boundary-kind:*`,
`instruction-simulation-recommended-action:*`, and
`instruction-simulation-artifact:*` signals plus
`simulation-priority:<priority>:<disposition>` signals so MDP/POMDP/neural
workers can learn which simulators, dry runs, and boundary decisions prevented
machine failure before hardware execution.

## `GET /fabrication/quality/catalog`

`GET /quality/catalog` and the gateway-prefixed
`GET /fabrication/quality/catalog` return the live
`dd.fabrication.quality-catalog.v1` inspection and metrology catalog before
callers treat generated or imported programs as machine-ready. The payload
exposes dimensional-metrology, additive-postprocess, subtractive-metrology,
sheet-cut-quality, assembly-quality, and traceability-quality contracts,
measurement contract targets, planning and instruction-analysis route aliases,
and response surfaces such as `qualityPlan.status`,
`qualityPlan.inspectionPoints`, `qualityPlan.measurementTargets`,
`qualityPlan.releaseGates`, `validation.failureBoundaries`,
`machineRelease.blockers`, `postprocessPlan.blockers`,
`releasePackagePlan.releaseGates`, and `interfaceControlPlan.controls`.
Catalog entries are inspection and measurement evidence contracts, not certified
acceptance results; machine-ready release remains blocked while required quality
inspection, postprocess, material traceability, interface fit, or metrology
evidence is absent. Quality observations are retained for MDP/POMDP/neural
workers so future planning can learn when to add inspection, split parts, adjust
processes, or require human signoff.

## `GET /fabrication/quality/preflight/catalog`

`GET /quality/preflight/catalog` and the gateway-prefixed
`GET /fabrication/quality/preflight/catalog` return the live
`dd.fabrication.quality-preflight-catalog.v1` quality-release checklist before
parts are assembled, packed, handed to a human, or marked machine-ready. The
catalog groups metrology instrument/datum state, first-article/final-fit/surface
state, and nonconformance/disposition/learning state. It calls out calibrated
instrument and CMM/vision/scan setup evidence, datum scheme, uncertainty,
artifact checksum, first article, final fit, surface finish, edge quality,
cleanliness, material-process witness evidence, interface gauges, torque/leak
or functional proof, sampling plans, nonconformance ownership, rework/remake or
waiver disposition, and learning observations.

Quality preflight entries are release evidence contracts, not certified
acceptance results. Failed checks should be retained through quality,
disposition, release, and learning outcome routes so DES, MDP/POMDP, and neural
workers can learn when to add inspection, split parts, reroute manufacturing, or
require human intervention earlier.

## `POST /fabrication/quality/plan`

`POST /quality/plan` and the gateway-prefixed
`POST /fabrication/quality/plan` accept the same request body as
`POST /fabrication/plan`, apply bounded learning-policy memory, run the planner,
store and publish the full plan result, and return a compact
`dd.fabrication.quality-planning.v1` inspection and metrology package. The
response focuses on `qualityPlan.status`, `qualityPlan.inspectionPoints`,
`qualityPlan.inspectionPoints.recordsToCapture`,
`qualityPlan.measurementTargets`, `qualityPlan.releaseGates`,
`qualityPlan.learningObservations`, `postprocessPlan.requiredArtifacts`,
`postprocessPlan.blockers`, `releasePackagePlan.packages`,
`releasePackagePlan.releaseGates`, `interfaceControlPlan.controls`,
`machineRelease.blockers`, `simulation.riskProfile`, and
`learning.releaseProbePlan`.

Quality plans are draft inspection and metrology evidence packages, not
certified acceptance results or shop-floor release authorization. Machine-ready
release remains blocked while required inspection, postprocess, traceability,
interface-fit, release-package, or metrology evidence is absent. Stored
artifacts include `quality-plan`, `postprocess-plan`, `release-package-plan`,
`machine-release`, `simulation-report`, and `mdp-request` so
MDP/POMDP/neural workers can learn when to add inspection, split parts, adjust
processes, regenerate instructions, or require human signoff.

## `GET /fabrication/dispositions/catalog`

`GET /dispositions/catalog` and the gateway-prefixed
`GET /fabrication/dispositions/catalog` return the live
`dd.fabrication.disposition-catalog.v1` post-inspection, post-simulation, and
post-failure decision catalog before callers treat quality results, failure
events, or release packages as closed. The catalog covers pass-with-retained
evidence, rework-and-reinspect, scrap-and-remake, engineering waiver/use-as-is,
and split/combine redesign dispositions.

Each disposition family lists decision evidence, release blockers, response
surfaces, and learning signals. The response names surfaces such as
`qualityResult.measurements`, `qualityResult.findings`, `simulation.findings`,
`failureModeResult.failureEvents`, `boundaryRemediationPlan.actions`,
`decompositionPlan.parts`, `interfaceControlPlan.interfaces`,
`assemblyPlan.requiredEvidence`, `releasePackagePlan.releaseGates`,
`learning.outcomes`, and `machineRelease.blockers`. Catalog entries are
decision evidence contracts, not certified quality acceptance. Machine-ready or
customer release remains blocked while pass, rework, scrap, waiver, or
split/combine redesign decisions lack retained evidence and human or automation
authority. Disposition outcomes are retained as MDP/POMDP/neural learning
signals so future planners can avoid failed routes, change fixtures, split
parts, remake, or add inspection earlier.

## `POST /fabrication/dispositions/result`

`POST /dispositions/result` and the gateway-prefixed
`POST /fabrication/dispositions/result` accept retained disposition outcomes
from quality, simulation, failure, release, or remediation reviewers. The
response uses `dd.fabrication.disposition-result-review.v1` and captures
pass/rework/scrap/waiver/split-combine decisions, remediation and reinspection
actions, authority/signoff reviews, retained artifacts, release blockers, and
MDP/POMDP/neural learning observations, plus a
`dd.fabrication.disposition-learning-outcome-draft.v1` payload that callers can
send to `POST /fabrication/learning/outcomes`.

Machine-ready and release-ready status remains blocked while a disposition
decision is missing, unaccepted, pending rework/remake/reinspection, waiting on
engineering/operator authority, or lacking artifact evidence. The result is
stored with `disposition-result`, `disposition-decisions`,
`disposition-remediation-actions`, `disposition-authority-reviews`,
`disposition-artifacts`, and `disposition-learning-observations` artifacts so
future planners can learn when to rework, remake, waive, split/combine, add
inspection, or avoid the failed route.

## `GET /fabrication/costing/catalog`

`GET /costing/catalog` and the gateway-prefixed
`GET /fabrication/costing/catalog` return the live
`dd.fabrication.costing-catalog.v1` estimation evidence catalog before callers
treat a plan, quote, schedule, or split/combine route as economically ready. The
catalog covers machine-time/setup estimates, material yield and scrap allowance,
quality/rework/release risk, split/combine route economics, and controller,
postprocessor, and artifact review effort.

Each cost family lists estimation evidence, release blockers, response
surfaces, and learning signals. The response names surfaces such as
`machineSchedule.lanes`, `materialPlan.quantity`, `qualityPlan.releaseGates`,
`boundaryRemediationPlan.actions`, `decompositionPlan.parts`,
`assemblyPlan.requiredEvidence`, `releasePackagePlan.requiredArtifacts`,
`learning.outcomes`, and `machineRelease.blockers`. Catalog entries are
estimation evidence contracts, not binding quotes, certified cost accounting, or
shop-floor release authorization. Machine-ready and customer release remain
blocked when route economics omit setup, material yield, scrap, quality, review,
human intervention, or split/combine evidence. Cost, yield, scrap, cycle-time,
and rework outcomes are retained as MDP/POMDP/neural learning signals so future
planners can choose cheaper, safer, or more reliable fabrication routes.

## `POST /fabrication/costing/result`

`POST /costing/result` and the gateway-prefixed
`POST /fabrication/costing/result` accept retained cost, yield, scrap,
cycle-time, rework, and split/combine route outcome reviews from generated,
imported, simulated, or completed fabrication work. The response uses
`dd.fabrication.costing-result-review.v1` and captures estimate families,
material-yield reviews, route comparisons, artifacts, release blockers,
MDP/POMDP/neural learning observations, and a
`dd.fabrication.costing-learning-outcome-draft.v1` payload that callers can send
to `POST /fabrication/learning/outcomes`.

Machine-ready and customer-ready release remain blocked while setup, material
yield, scrap, quality, review, human-intervention, or split/combine route
economics lack retained evidence. The result is stored with `costing-result`,
`costing-reviews`, `costing-yield-reviews`, `costing-route-comparisons`,
`costing-artifacts`, and `costing-learning-observations` artifacts so future
planners can learn which one-piece, split, hybrid, rework, or remake routes are
cheaper, safer, and reliable enough for release.

## `GET /fabrication/utilities/catalog`

`GET /utilities/catalog` and the gateway-prefixed
`GET /fabrication/utilities/catalog` return the live
`dd.fabrication.utilities-catalog.v1` process-support and facility-readiness
catalog before callers treat generated, imported, simulated, or scheduled work
as machine-ready. The catalog covers additive thermal/material utilities,
subtractive coolant/chip/dust/air support, sheet-cut process support, hybrid
cell fixture and robot services, and facility power, network, and environment
readiness.

Each utility family lists required evidence, release blockers, response
surfaces, and learning signals. The response names surfaces such as
`validation.failureBoundaries`, `supportStrategyPlan.requirements`,
`monitoringPlan.alerts`, `fixturePlan.setups`, `toolingPlan.requirements`,
`executionPlan.stopPoints`,
`operatorInterventionPlan.requiredOperatorActions`, `scheduleResult.holds`,
`learning.outcomes`, and `machineRelease.blockers`. Catalog entries are
process-support and facility-readiness evidence contracts, not certified machine
safety approval or facility compliance. Machine-ready release remains blocked
while power, network, thermal, material-supply, coolant, chip, dust, gas, pump,
abrasive, fume, vacuum, fixture, robot, or recovery utilities lack retained
evidence. Utility outages, restarts, operator recovery, and environmental
excursions are retained as MDP/POMDP/neural learning signals so future planners
can resequence, add checkpoints, split work, or avoid brittle unattended routes.

## `POST /fabrication/utilities/result`

`POST /utilities/result` and the gateway-prefixed
`POST /fabrication/utilities/result` accept retained process-support and
facility-readiness outcomes from coolant, chip evacuation, dust collection,
sheet-cut support media, additive thermal/material utilities, hybrid-cell
services, power, network, environment, and operator recovery reviewers. The
response uses `dd.fabrication.utilities-result-review.v1` and captures utility
checks, recovery actions, outage events, retained artifacts, release blockers,
warning counts, and MDP/POMDP/neural learning observations plus a
`dd.fabrication.utilities-learning-outcome-draft.v1` payload that callers can
send to `POST /fabrication/learning/outcomes`.

Machine-ready and release-ready status remains blocked when process support
utilities are unavailable, outside limits, unrecovered, restart-unverified,
unreplanned, missing retained evidence, or still require human intervention.
The result is stored with `utilities-result`, `utilities-checks`,
`utilities-recovery-actions`, `utilities-outage-events`, `utilities-artifacts`,
and `utilities-learning-observations` artifacts so future planners can learn
which utility readiness, restart, outage, and recovery patterns made generated
or imported instructions releasable.

## `GET /fabrication/energy/catalog`

`GET /energy/catalog` and the gateway-prefixed
`GET /fabrication/energy/catalog` return the live
`dd.fabrication.energy-catalog.v1` machine, process, and facility power
evidence catalog before generated, imported, simulated, scheduled, or hybrid
fabrication work is treated as machine-ready. The catalog covers additive
heater/motion/build energy, subtractive spindle/axis/coolant load,
sheet-cutting beam/jet/plasma/EDM energy, and facility grid, UPS, and carbon
window evidence.

Each energy family lists required evidence, release blockers, response
surfaces, and learning signals. The response names surfaces such as
`scheduleResult.lanes`, `costingResult.estimateFamilies`,
`utilitiesResult.checks`, `availabilityResult.capacityWindows`,
`monitoringPlan.alerts`, `telemetryResult.channels`, `machineRelease.blockers`,
and `learning.outcomes`. Catalog entries are power-readiness evidence
contracts, not utility billing, certified electrical design, or carbon
compliance approval. Machine-ready release remains blocked while heater,
spindle, axis, beam, jet, pump, compressor, chiller, UPS, facility circuit, or
thermal-load evidence is missing for the selected route. Energy outcomes are
retained as costing, availability, schedule, maintenance, and MDP/POMDP/neural
learning signals so future planners can split, combine, defer, or reroute
brittle fabrication work.

## `POST /fabrication/energy/result`

`POST /energy/result` and the gateway-prefixed
`POST /fabrication/energy/result` accept retained machine, process, and
facility energy outcome reviews from generated, imported, simulated, scheduled,
or completed fabrication work. The response uses
`dd.fabrication.energy-result-review.v1` and captures power checks, thermal-load
checks, recovery actions, artifacts, release blockers, and MDP/POMDP/neural
learning observations plus a
`dd.fabrication.energy-learning-outcome-draft.v1` payload that callers can send
to `POST /fabrication/learning/outcomes`.

Machine-ready and release-ready status remains blocked while selected routes
lack verified heater, spindle, axis, beam, jet, pump, compressor, chiller, UPS,
facility circuit, carbon-window, or thermal-load evidence. Power overloads,
thermal duty-cycle limits, incomplete recovery actions, missing retained
artifacts, required operator reviews, and split/combine or defer decisions are
stored with `energy-result`, `energy-power-checks`, `energy-thermal-checks`,
`energy-recovery-actions`, `energy-artifacts`, and
`energy-learning-observations` artifacts so future planners can learn which
power budgets, cooldown windows, batch schedules, and route choices made work
releasable.

## `GET /fabrication/telemetry/catalog`

`GET /telemetry/catalog` and the gateway-prefixed
`GET /fabrication/telemetry/catalog` return the live
`dd.fabrication.telemetry-catalog.v1` runtime evidence catalog before execution,
monitoring, simulation, or learning workers turn sensor streams into release or
policy-training outcomes. The catalog covers additive print runtime sensors,
subtractive load/vibration/process state, sheet-cut process and support-media
telemetry, hybrid-cell assembly and handoff telemetry, and simulation-to-runtime
boundary correlation.

Each telemetry family lists required channels, release blockers, response
surfaces, and learning signals. The response names surfaces such as
`executionTelemetryResult.telemetryArtifacts`,
`executionTelemetryResult.machineStops`,
`executionTelemetryResult.operatorInterventions`, `monitoringResult.alerts`,
`failureModeResult.failureEvents`, `qualityResult.measurements`,
`simulation.findings`, `validation.failureBoundaries`, and `learning.outcomes`.
Catalog entries are runtime evidence contracts, not certified machine safety
validation or calibrated metrology acceptance. Machine-ready and learning-ready
release remain blocked when sensor streams, machine stops, operator
interventions, generated/imported program hashes, or boundary-correlation
evidence cannot be retained. Telemetry outcomes feed MDP/POMDP/neural workers so
future planners can learn which generated instructions, machine choices,
utilities, split/combine handoffs, and human checkpoints prevented or caused
failures.

## `GET /fabrication/availability/catalog`

`GET /availability/catalog` and the gateway-prefixed
`GET /fabrication/availability/catalog` return the live
`dd.fabrication.availability-catalog.v1` capacity and readiness evidence catalog
before machine selection, scheduling, generated/imported instructions, or
unattended release are treated as shop-ready. The catalog covers live machine
state and queue capacity, material/fixture/tooling/utility readiness,
operator/automation coverage, and cross-machine split/combine capacity.

Each availability family lists required evidence, release blockers, response
surfaces, artifact surfaces, and learning signals. The response names surfaces
such as `machineSelection.candidates`, `machineSchedule.machineLanes`,
`scheduleResult.holds`, `materialPlan.routeRequirements`,
`toolingPlan.requirements`, `utilitiesResult.checks`,
`operatorInterventionPlan.requiredOperatorActions`, `machineRelease.blockers`,
and `learning.outcomes`. Catalog entries are capacity and readiness evidence
contracts, not certified shop scheduling authority or guaranteed machine uptime.
Machine-ready and unattended release remain blocked while live machine state,
queue, material, tooling, fixture, utility, maintenance, operator, or automation
capacity evidence is stale or missing. Availability outcomes feed DES,
MDP/POMDP/neural workers so future planners can learn fallback machines,
split/combine capacity, queue-delay risk, and reliable unattended windows.

## `POST /fabrication/availability/result`

`POST /availability/result` and the gateway-prefixed
`POST /fabrication/availability/result` accept retained availability worker
outcomes for machine windows, queue/capacity state, material/fixture/tooling and
utility readiness, operator/automation coverage, fallback machines, and
split/combine capacity. The response uses
`dd.fabrication.availability-result-review.v1` and includes a
`dd.fabrication.availability-learning-outcome-draft.v1` payload that callers can
send to `POST /fabrication/learning/outcomes`.

Machine-ready and unattended release remain blocked while selected machines lack
current online/queue/setup/downtime evidence, resources are unavailable,
operator or automation windows are missing, fallback machines are not viable, or
split/combine capacity evidence is absent. The result is stored with
`availability-result`, `availability-machine-windows`,
`availability-resource-checks`, `availability-fallback-options`,
`availability-artifacts`, and `availability-learning-observations` artifacts so
DES/MDP/POMDP/neural planners can learn fallback-machine, queue-delay,
operator-window, and split/combine capacity outcomes.

## `GET /fabrication/maintenance/catalog`

`GET /maintenance/catalog` and the gateway-prefixed
`GET /fabrication/maintenance/catalog` return the live
`dd.fabrication.maintenance-catalog.v1` service-readiness evidence catalog
before generated, imported, scheduled, or unattended work is treated as
machine-ready. The catalog covers lockout/tagout release, spindle/tooling wear,
thermal/fluid/process-support service, and calibration/sensor/safety-channel
service state.

Each maintenance family lists required evidence, release blockers, response
surfaces, artifact surfaces, and learning signals. The response names surfaces
such as `machineProfile.evidence.maintenance`, `setupResult.datumReviews`,
`calibrationResult.probeReviews`, `utilitiesResult.checks`,
`monitoringResult.alerts`, `telemetryResult.boundaryCorrelations`,
`safetyResult.interlocks`, `machineRelease.blockers`, and `learning.outcomes`.
Catalog entries are service-readiness evidence contracts, not certified
maintenance approval or regulatory lockout procedure. Machine-ready, unattended,
and customer-ready release remain blocked while lockout, service, wear,
calibration, sensor, process-support, or safety-channel evidence is stale or
missing. Maintenance outcomes are retained as MDP/POMDP/neural learning signals
so future planners can avoid brittle machines, add operator checkpoints, split
work across healthier equipment, or schedule service before release.

## `POST /fabrication/maintenance/result`

`POST /maintenance/result` and the gateway-prefixed
`POST /fabrication/maintenance/result` accept retained maintenance worker
outcomes for service items, lockout clearances, post-service verification
checks, residual restrictions, retained artifacts, and release warnings. The
response uses `dd.fabrication.maintenance-result-review.v1` and includes a
`dd.fabrication.maintenance-learning-outcome-draft.v1` payload that callers can
send to `POST /fabrication/learning/outcomes`.

Machine-ready and unattended release remain blocked while service state is
overdue, lockout/tagout clearance lacks authorization, post-service dry-run,
homing, interlock, safe-stop, sensor, or datum checks fail, residual
restrictions require an operator, or artifacts lack URI, checksum, format, and
evidence labels. The result is stored with `maintenance-result`,
`maintenance-service-items`, `maintenance-lockout-clearances`,
`maintenance-verification-checks`, `maintenance-residual-restrictions`,
`maintenance-artifacts`, and `maintenance-learning-observations` artifacts so
MDP/POMDP/neural planners can learn brittle-machine, service-schedule,
operator-checkpoint, and route-across-healthier-equipment outcomes.

## `POST /fabrication/telemetry/result`

`POST /telemetry/result` and the gateway-prefixed
`POST /fabrication/telemetry/result` accept retained runtime telemetry outcomes
from additive, subtractive, sheet-cutting, hybrid-cell, and simulation-boundary
workers. The response uses `dd.fabrication.telemetry-result-review.v1` and
captures sensor windows, machine stops, boundary correlations, operator
interventions, retained artifacts, release blockers, warning counts, and
MDP/POMDP/neural learning observations plus a
`dd.fabrication.telemetry-learning-outcome-draft.v1` payload that callers can
send to `POST /fabrication/learning/outcomes`.

Machine-ready and learning-ready status remains blocked when telemetry samples
are not retained, sensor windows drift outside the reviewed envelope, machine
stops lack safe-stop/restart evidence, predicted boundaries do not match actual
runtime events, operator interventions remain incomplete, or artifacts lack URI,
checksum, format, and evidence labels. The result is stored with
`telemetry-result`, `telemetry-sensor-windows`, `telemetry-machine-stops`,
`telemetry-boundary-correlations`, `telemetry-operator-interventions`,
`telemetry-artifacts`, and `telemetry-learning-observations` artifacts so future
planners can learn which generated/imported instructions, machine choices,
utilities, split/combine handoffs, and human checkpoints prevented or caused
runtime failures.

## `POST /fabrication/quality/result`

`POST /quality/result` and the gateway-prefixed
`POST /fabrication/quality/result` accept inspection and metrology worker
results, normalize them into
`dd.fabrication.quality-result-review.v1`, and store a bounded review job with
retained measurement, finding, gate, artifact, and learning-observation
surfaces plus a `dd.fabrication.quality-learning-outcome-draft.v1` payload that
callers can send to `POST /fabrication/learning/outcomes`. The response reports
blocker counts for out-of-tolerance
measurements, nonconformance or human-intervention findings, unresolved
inspection gates, missing artifact evidence, and a `priorityDispositions` array
for machine-failure, human-intervention, split/combine or interface,
non-G-code/job-sheet evidence, and learning-feedback lanes.

Quality result reviews are retained release evidence, not certified acceptance
or a machine-safety waiver. Machine-ready release remains blocked until
measurements, findings, gates, artifacts, and any human dispositions clear.
Stored artifacts include `quality-result`, `quality-measurements`,
`quality-findings`, `quality-inspection-gates`, `quality-artifacts`,
`quality-priority-dispositions`, and `quality-learning-observations`. Quality
observations include `quality-priority:<priority>:<disposition>` signals so
MDP/POMDP/neural workers can learn when to adjust processes, split or combine
parts, add inspection, or require human signoff before release.

## `GET /fabrication/calibration/catalog`

`GET /calibration/catalog` and the gateway-prefixed
`GET /fabrication/calibration/catalog` return the live
`dd.fabrication.calibration-catalog.v1` homing, work-offset, tool-length,
probe, thermal, process-media, sensor, and fixture calibration evidence catalog
before callers treat generated or imported work as machine-ready. The payload
exposes additive homing/bed/hotend, subtractive work-offset/tool-length, lathe
offset/spindle/support, sheet-cut process-origin/media, and robotic
assembly-fixture/vision calibration contracts, planning and
instruction-analysis route aliases, and response surfaces such as
`machineProfile.profileEvidence.calibration`, `fixturePlan.setups.datumScheme`,
`toolingPlan.requirements.setupChecks`, `releaseProbePlan.probes`,
`validation.failureBoundaries`, `machineRelease.blockers`,
`improvedPrograms.patchManifest.operations`, and `monitoringPlan.monitorPoints`.
Catalog entries are calibration evidence contracts, not certified machine
calibration procedures; machine-ready release remains blocked while homing, work
offset, tool length, thermal, process-media, sensor, or fixture calibration
evidence is absent. Calibration observations are retained for MDP/POMDP/neural
workers so future planning can learn when to request probes, split jobs, add
operator checkpoints, or regenerate instructions.

## `POST /fabrication/calibration/plan`

`POST /calibration/plan` and the gateway-prefixed
`POST /fabrication/calibration/plan` accept the same request body as
`POST /fabrication/plan`, apply bounded learning-policy memory, run the planner,
store and publish the full plan result, and return a compact
`dd.fabrication.calibration-planning.v1` calibration-readiness package. The
response focuses on `learning.releaseProbePlan`, `releaseProbePlan.probes`,
`releaseProbePlan.probes.requiredBeforeState`,
`releaseProbePlan.requiredBeforeRelease`, `machineRelease.checklist`,
`machineRelease.blockers`, `toolingPlan.requirements.setupChecks`,
`fixturePlan.setups.datumScheme`, `fixturePlan.setups.requiredEvidence`,
`monitoringPlan.monitorPoints`, `simulation.programs`, and
`validation.failureBoundaries`.

Calibration plans are draft homing, work-offset, tool-length, probe, thermal,
process-media, fixture, and monitoring evidence packages, not certified
calibration procedures or machine-safety approvals. Machine-ready release remains
blocked while release probes, datum transfer, tool length, fixture, thermal,
sensor, support-media, or process calibration evidence is unresolved. Stored
artifacts include `release-probe-plan`, `machine-release`, `tooling-plan`,
`fixture-plan`, `monitoring-plan`, `simulation-report`, and `mdp-request` so
MDP/POMDP/neural workers can learn when to require probes, improve machine
profiles, split jobs, add operator checkpoints, or regenerate instructions.

## `POST /fabrication/calibration/result`

`POST /calibration/result` and the gateway-prefixed
`POST /fabrication/calibration/result` accept calibration worker results,
normalize them into `dd.fabrication.calibration-result-review.v1`, and store a
bounded review job with retained check, offset, probe, artifact, and
learning-observation surfaces plus a
`dd.fabrication.calibration-learning-outcome-draft.v1` payload that callers can
send to `POST /fabrication/learning/outcomes`. The response reports blocker counts for failed or
human-intervention checks, out-of-tolerance offsets, unresolved release probes,
missing calibration artifact evidence, and a `priorityDispositions` array for
machine-failure, human-intervention, split/combine or setup/interface,
non-G-code/job-sheet evidence, and learning-feedback lanes.

Calibration result reviews are retained machine-preparation evidence, not
certified calibration or a machine-safety waiver. Machine-ready release remains
blocked until required homing, work-offset, tool-length, thermal, process-media,
sensor, fixture, probe, artifact, and human-disposition evidence clears. Stored
artifacts include `calibration-result`, `calibration-checks`,
`calibration-offsets`, `calibration-probes`, `calibration-artifacts`,
`calibration-priority-dispositions`, and `calibration-learning-observations`.
Calibration observations include `calibration-priority:<priority>:<disposition>`
signals so MDP/POMDP/neural workers can learn when to request probes, split
setups, add operators, improve machine profiles, or regenerate instructions
before release.

## `GET /fabrication/interventions/catalog`

`GET /interventions/catalog` and the gateway-prefixed
`GET /fabrication/interventions/catalog` return the live
`dd.fabrication.intervention-catalog.v1` operator-intervention and automation
catalog before callers treat a plan or imported program as unattended-run
eligible. The payload exposes action contracts, automation types, evidence-gate
contracts, planning and instruction-analysis route aliases, and response surfaces
such as `boundarySummary.automationRequirements`,
`interventionMap.humanInterventionPoints`, `interventionMap.automationPaths`,
`interventionMap.splitCombineDecisions`, `executionPlan.stopPoints`,
`operatorInterventionPlan.requiredOperatorActions`,
`operatorInterventionPlan.evidenceGates`,
`operatorInterventionPlan.automationCandidates`,
`operatorInterventionPlan.splitCombineReviews`, `releaseProbePlan.probes`, and
`pomdpBeliefState.hiddenStates`. Catalog entries are preflight evidence
contracts, not controller-certified restart instructions; machine-ready release
remains blocked while required operator actions, unresolved execution stop
points, split/combine reviews, or unverified automation candidates remain open.
Human-intervention and automation observations are emitted for MDP/POMDP/neural
workers so future planning can learn when to add automation, split jobs, or keep
human checkpoints.

## `POST /fabrication/interventions/result`

`POST /interventions/result` and the gateway-prefixed
`POST /fabrication/interventions/result` accept retained
`dd.fabrication.intervention-result-review.v1` reviews from operator,
automation, execution, monitoring, and split/combine workers after an
intervention gate is attempted or cleared.

The review blocks machine-ready release when operator actions are incomplete,
automation handoffs are unverified or require fallback, split/combine interface
or recomposition evidence is missing, evidence gates are unacknowledged, or
retained artifacts lack URI, checksum, format, and evidence labels. The response
also includes a `priorityDispositions` array for machine-failure,
human-intervention, split/combine or interface, non-G-code/job-sheet evidence,
and learning-feedback lanes. Accepted responses store `intervention-result`,
`intervention-operator-actions`,
`intervention-automation-handoffs`, `intervention-split-combine-reviews`,
`intervention-evidence-gates`, `intervention-artifacts`, and
`intervention-priority-dispositions`, and `intervention-learning-observations`
artifacts. They also emit
`dd.fabrication.intervention-learning-outcome-draft.v1` so MDP/POMDP/neural
workers can learn which human checkpoints, automation substitutions,
split/combine reviews, and restart paths cleared or blocked release. Learning
observations include `intervention-priority:<priority>:<disposition>` signals
so blocked operator, automation, evidence, and recomposition lanes can train
future routing decisions.

## `GET /fabrication/setup/catalog`

`GET /setup/catalog` and the gateway-prefixed
`GET /fabrication/setup/catalog` return the live
`dd.fabrication.setup-catalog.v1` tooling, fixture, datum, workholding, runtime
monitoring, and recovery evidence catalog before callers treat generated or
imported work as machine-ready. The payload exposes setup contracts for additive
build setup, mill/router tooling and fixtures, lathe, mill-turn, and Swiss guide-bushing/bar-feed grip/support,
sheet-cut support media, assembly/recomposition datum transfer, and unattended
run monitoring. It names response surfaces such as
`toolingPlan.requirements`, `toolingPlan.requirements.requiredTools`,
`toolingPlan.requirements.workholding`, `fixturePlan.setups`,
`fixturePlan.setups.datumScheme`, `fixturePlan.setups.clearanceChecks`,
`fixturePlan.datumTransfers`, `monitoringPlan.monitorPoints`,
`monitoringPlan.alertRules`, and `monitoringPlan.recoveryActions`, plus retained
artifact surfaces `tooling-plan`, `fixture-plan`, `monitoring-plan`, and the
matching `mdp-request` artifact payloads. Catalog entries are setup evidence
contracts, not certified fixture designs or safety procedures; machine-ready
release remains blocked while required tools, workholding, setup checks, datum
transfer, fixture evidence, monitoring channels, alert rules, recovery actions,
or operator/automation signoff gates are unresolved. Setup, fixture, and
monitoring observations are retained for MDP/POMDP/neural workers so future
planning can learn when to change workholding, split setups, add automation, or
require human intervention.

## `GET /fabrication/tooling/catalog`

`GET /tooling/catalog` and the gateway-prefixed
`GET /fabrication/tooling/catalog` return the live
`dd.fabrication.tooling-catalog.v1` machine-tooling evidence catalog before
generated or imported instructions are treated as ready for toolpath, controller,
simulation, or release handoff. The catalog covers additive extrusion tooling,
subtractive cutters/holders/probes, lathe inserts/turrets/support tooling,
sheet-cutting process consumables, and inspection/probing/calibration tools for
FDM, pellet/FGF, robotic additive, vertical and horizontal mills, five-axis and
rotary mills, routers, lathes, mill-turn and Swiss machines, laser/waterjet/plasma
and wire-EDM sheet cutting, and CMM-style inspection cells.

Each tooling family lists tool evidence, release blockers, response surfaces,
and learning signals. The response names surfaces such as
`toolingPlan.requirements.requiredTools`, `toolingPlan.requirements.consumables`,
`toolingPlan.releaseGates`, `fixturePlan.setups.requiredEvidence`,
`controllerPlan.requiredControllerChecks`, `calibrationPlan.offsetEvidence`,
`qualityPlan.measurementTargets`, and `machineRelease.blockers`. Catalog entries
are required tool, consumable, offset, holder, probe, and support evidence
contracts, not certified tooling setup sheets. Machine-ready release remains
blocked until tool identity, geometry, offsets, wear/tool-life, process support,
calibration, and operator or automation signoff evidence clear. Tool selection,
tool-life, offset, feed/speed, support-media, and inspection outcomes are retained
as MDP/POMDP/neural learning signals for future planning and instruction repair.

## `POST /fabrication/tooling/result`

`POST /tooling/result` and the gateway-prefixed
`POST /fabrication/tooling/result` accept retained tooling worker outcomes for
tool identity, holder/station state, geometry, controller offsets, tool life,
wear, support media, consumables, probe/tool-setting evidence, retained
artifacts, warnings, and review metadata. The response uses
`dd.fabrication.tooling-result-review.v1` and emits a
`dd.fabrication.tooling-learning-outcome-draft.v1` `outcomeDraft` with tool, offset, tool-life, support-media, artifact, blocker, human-intervention, and release-state feature hints for MDP/POMDP/neural learners.

Machine-ready and unattended release remain blocked while tool checks, offset
checks, tool-life margins, coolant/gas/abrasive/wire/probe support media,
artifact checksums, or human-review gates are unresolved. Stored artifacts
include `tooling-result`, `tooling-tool-checks`, `tooling-offset-checks`,
`tooling-tool-life-checks`, `tooling-support-media-checks`,
`tooling-artifacts`, and `tooling-learning-observations` so future planners can
change tools, split setups, refresh offsets, schedule tool replacement, add
probes, restart process support, or require human intervention earlier.

## `GET /fabrication/consumables/catalog`

`GET /consumables/catalog` and the gateway-prefixed
`GET /fabrication/consumables/catalog` return the live
`dd.fabrication.consumables-catalog.v1` material, tool-life, support-media, and
process-consumable evidence catalog before generated/imported instructions or
unattended jobs are released. The catalog covers additive material and extrusion
consumables, subtractive cutters/inserts/coolant, sheet-cutting nozzle/gas/
abrasive/wire consumables, and resin, powder, binder, and postprocess
consumables.

Each consumable family lists required evidence, release blockers, response
surfaces, artifact surfaces, and learning signals. The response names surfaces
such as `materialPlan.routeRequirements`,
`toolingPlan.requirements.consumables`, `utilitiesResult.checks`,
`supportStrategyPlan.requirements`, `monitoringPlan.alerts`,
`qualityResult.measurements`, `postprocessPlan.requiredArtifacts`,
`provenanceResult.lineage`, `machineRelease.blockers`, and
`learning.outcomes`. Catalog entries are evidence contracts, not certified
inventory, tooling, or hazardous-material approval. Machine-ready and unattended
release remain blocked while material quantity, lot, shelf-life, dry state, tool
life, wear, nozzle, gas, abrasive, coolant, wire, resin, powder, binder, solvent,
media, or postprocess consumable evidence is stale or missing. Consumable
outcomes are retained as MDP/POMDP/neural learning signals so future planners can
learn tool-life risk, material capacity, support-media depletion, split/combine
reroutes, and operator refill checkpoints.

## `POST /fabrication/consumables/result`

`POST /consumables/result` and the gateway-prefixed
`POST /fabrication/consumables/result` accept retained consumables worker
outcomes for material/tool/support-media inventory, lot and shelf-life state,
remaining capacity, dry-state evidence, tool-life and wear checks, support-media
restart checks, artifacts, and release warnings. The response uses
`dd.fabrication.consumables-result-review.v1`.

Machine-ready and unattended release remain blocked while material, resin,
powder, binder, wire, gas, abrasive, coolant, solvent, media, or tooling capacity
is stale, depleted, below projected program demand, not lot-traceable, expired,
outside dry-state requirements, or missing support-media restart evidence. The
result is stored with `consumables-result`, `consumables-inventory-checks`,
`consumables-tool-life-checks`, `consumables-support-media-checks`,
`consumables-artifacts`, and `consumables-learning-observations` artifacts. Its
learning section includes a
`dd.fabrication.consumables-learning-outcome-draft.v1` `outcomeDraft` with
inventory, tool-life, support-media, artifact, split/combine, human-intervention,
and blocker hints ready for `POST /fabrication/learning/outcomes`, so
MDP/POMDP/neural planners can learn tool-life risk, material capacity,
support-media depletion, split/combine reroutes, and operator refill checkpoints.

## `GET /fabrication/workholding/catalog`

`GET /workholding/catalog` and the gateway-prefixed
`GET /fabrication/workholding/catalog` return the live
`dd.fabrication.workholding-catalog.v1` stock, build, fixture, support,
retention, and recomposition holding evidence catalog before generated or
imported instructions are treated as ready for setup, toolpath, simulation, or
release handoff. The catalog covers additive build plates, vats, powder beds,
sheet stacks, and build-surface reset evidence; mill/router vises, clamps,
vacuum tables, pallets, tombstones, tabs, spoilboards, and datum transfers;
lathe chucks, collets, guide bushings, supports, subspindles, and part-off
catchers; sheet-cut slats, honeycomb, water tables, retained tabs, nests, and
drop control; and hybrid assembly fixtures for split/combine recomposition.

Each workholding family lists machine kinds, holding evidence, release blockers,
response surfaces, and learning signals. The response names surfaces such as
`toolingPlan.requirements.workholding`, `fixturePlan.setups`,
`fixturePlan.setups.requiredEvidence`, `fixturePlan.setups.clearanceChecks`,
`fixturePlan.datumTransfers`, `simulation.riskProfile.programRisks`,
`operatorInterventionPlan.requiredOperatorActions`,
`interfaceControlPlan.interfaces`, `decompositionPlan.parts`,
`assemblyPlan.requiredEvidence`, `releasePackagePlan.requiredArtifacts`, and
`machineRelease.blockers`. Catalog entries are evidence contracts, not
certified fixture designs. Machine-ready release remains blocked while
build-surface, clamp, vacuum, chuck, support, tab, nest, datum-transfer, or
split/combine fixture evidence is unresolved. Workholding failures and
successful fixture choices are retained as MDP/POMDP/neural learning signals so
future planners can split jobs, change fixtures, add probes, or require human
intervention earlier.

## `GET /fabrication/workholding/preflight/catalog`

`GET /workholding/preflight/catalog` and the gateway-prefixed
`GET /fabrication/workholding/preflight/catalog` return the live
`dd.fabrication.workholding-preflight-catalog.v1` release checklist for fixture,
stock, build-surface, datum-transfer, clearance, split/combine fixture, and
operator handoff evidence before unattended motion or machine-ready release. The
catalog groups required evidence into stock/build-surface primary hold state,
datum-transfer/re-probe/clearance state, and split-combine fixture plus
human-intervention state.

Preflight entries describe required release evidence, not certified fixture
designs. Release remains blocked while stock, build plate, clamp, vacuum, chuck,
support, datum transfer, clearance, or split/combine fixture evidence is absent.
Failed checks feed DES, MDP/POMDP, and neural workers so future plans can split
jobs, add fixtures, insert re-probes, or require human intervention earlier.

## `POST /fabrication/workholding/result`

`POST /workholding/result` and the gateway-prefixed
`POST /fabrication/workholding/result` accept retained workholding worker
outcomes for fixture/build-surface/chuck/support checks, datum transfers,
clearance checks, split/combine fixture holds, retained artifacts, and release
warnings. The response uses `dd.fabrication.workholding-result-review.v1`.
Its learning section emits a
`dd.fabrication.workholding-learning-outcome-draft.v1` `outcomeDraft` with
fixture, datum-transfer, clearance, split/combine, artifact, human-intervention,
blocker, reward, and submit-route hints for `POST /fabrication/learning/outcomes`.

Machine-ready and unattended release remain blocked while fixture or build
surface holding is unverified, datum transfer or re-probe evidence is missing,
toolpath clearance intersects clamps, jaws, tabs, nests, or supports,
split/combine recomposition fixture evidence is unresolved, or artifacts lack
URI, checksum, format, and evidence labels. The result is stored with
`workholding-result`, `workholding-fixture-checks`,
`workholding-datum-transfers`, `workholding-clearance-checks`,
`workholding-split-combine-holds`, `workholding-artifacts`, and
`workholding-learning-observations` artifacts so MDP/POMDP/neural planners can
learn fixture failures, datum-transfer risk, clamp collision risk,
split/combine recomposition holds, and earlier human-intervention points.

## `GET /fabrication/nesting/catalog`

`GET /nesting/catalog` and the gateway-prefixed
`GET /fabrication/nesting/catalog` return the live
`dd.fabrication.nesting-catalog.v1` build-plate, powder-bed, sheet-cut,
flat-blank, and hybrid kit layout evidence catalog before generated or imported
jobs are treated as ready for design export, toolpath, simulation, setup,
postprocess, assembly, or release handoff.

The catalog covers FDM/resin/material-jet build-plate orientation and batch
traceability; SLS/MJF/metal-PBF/binder-jet packing, thermal spacing, recoater
clearance, depowder, stress-relief, and plate-removal layouts; laser/waterjet/
plasma/wire-EDM/hot-wire sheet nests with kerf, pierce/thread, tabs, bridges,
drop control, skeleton handling, and support-media evidence; press-brake flat
blank, grain direction, tooling, tonnage, bend sequence, and formed-part
traceability; and hybrid split/combine kit layout for printed, milled, turned,
sheet-cut, postprocessed, and assembled subparts.

Each nesting family lists layout evidence, release blockers, response surfaces,
artifact surfaces, and learning signals. The response names surfaces such as
`designExports.partExports.content.nesting`, `slicerPlan.profileEvidence`,
`toolingPlan.requirements.consumables`, `fixturePlan.setups.workholding`,
`supportStrategyPlan`, `postprocessPlan.steps`, `executionPlan.stopPoints`,
`qualityPlan.measurementTargets`, `releasePackagePlan.packages`, and
`machineRelease.blockers`. Catalog entries are evidence contracts, not certified
CAM or slicer nests. Machine-ready release remains blocked while layout envelope,
material, support, tab/drop, thermal, traceability, fixture, postprocess, or
operator/automation evidence is unresolved. Nesting observations are retained
for MDP/POMDP/neural workers so future plans can adjust orientation, split jobs,
change batch layout, add retention, or require human intervention earlier.

## `POST /fabrication/nesting/result`

`POST /nesting/result` and the gateway-prefixed
`POST /fabrication/nesting/result` accept retained
`dd.fabrication.nesting-result-review.v1` reviews from nesting, slicer, CAM,
sheet-cutting, powder-bed packing, split/combine, or operator workers after
layout, traceability, retention, and kit recomposition evidence is checked
against the selected machine route.

The review blocks machine-ready release when worker execution failed, layout or
traceability checks are missing, envelope/material/batch placement is
unverified, tabs/bridges/slugs/drop zones or thermal spacing remain risky,
split/combine kit labels cannot be recomposed, human intervention is required,
or retained artifacts lack URI, checksum, format, and evidence labels. Accepted
responses store `nesting-result`, `nesting-layout-checks`,
`nesting-traceability-checks`, `nesting-retention-checks`,
`nesting-split-combine-holds`, `nesting-artifacts`, and
`nesting-learning-observations` artifacts so MDP/POMDP/neural planners can learn
bad nests, missing batch traceability, drop-control failures, split/combine
holds, and earlier human-intervention points.

## `GET /fabrication/support-strategies/catalog`

`GET /support-strategies/catalog` and the gateway-prefixed
`GET /fabrication/support-strategies/catalog` return the live
`dd.fabrication.support-strategy-catalog.v1` orientation, support,
sacrificial-holding, tab, bridge, split/combine, and support-removal evidence
catalog before generated or imported instructions are treated as ready for
design generation, setup, simulation, assembly, or release handoff. The catalog
covers additive build orientation, overhangs, bridges, islands, support
interfaces, support removal, depowder, wash/cure, and plate removal;
subtractive setup orientation, tabs, bridges, soft jaws, sacrificial stock, and
multi-sided support; turning stick-out, guide-bushing, tailstock, subspindle,
catcher, and cutoff support; sheet-cut nests, tabs, bridges, drop support, and
skeleton handling; and hybrid split/combine interface and recomposition support.

Each support-strategy family lists machine kinds, strategy evidence, release
blockers, response surfaces, and learning signals. The response names surfaces
such as `designInputReview.manufacturabilityEvidence`, `slicerProfileCatalog`,
`fixturePlan.setups`, `toolingPlan.requirements.workholding`,
`decompositionPlan.parts`, `interfaceControlPlan.interfaces`,
`interventionMap.requiredInterventions`,
`interventionMap.splitCombineDecisions`, `executionPlan.stopPoints`,
`postprocessPlan.requiredArtifacts`, `qualityPlan.measurementTargets`,
`learning.outcomes`, and `machineRelease.blockers`. Catalog entries are
evidence contracts, not certified manufacturing instructions. Machine-ready
release remains blocked while orientation, supports, tabs, bridges, sacrificial
stock, support removal, postprocess access, or split/combine interface evidence
is unresolved. Orientation, support, split/combine, and intervention outcomes
are retained as MDP/POMDP/neural learning signals so future planners can choose
one-piece, split, combine, or alternate-machine routes earlier.

## `POST /fabrication/support-strategies/result`

`POST /support-strategies/result` and the gateway-prefixed
`POST /fabrication/support-strategies/result` accept retained
`dd.fabrication.support-strategy-result-review.v1` reviews from slicer, CAM,
simulation, decomposition, assembly, or operator workers after orientation,
support, sacrificial-holding, tab, bridge, or split/combine strategies are
checked against the requested machine route.

The request records orientation reviews, support/removal reviews,
split/combine decisions, human interventions, artifacts, warnings, and learning
observations. Responses retain `support-strategy-result`,
`support-strategy-orientation-reviews`, `support-strategy-support-reviews`,
`support-strategy-split-combine-decisions`, `support-strategy-interventions`,
`support-strategy-artifacts`, and `support-strategy-learning-observations`
artifacts on the fabrication job. The learning section includes a
`dd.fabrication.support-strategy-learning-outcome-draft.v1` `outcomeDraft` with
orientation, support, split/combine, intervention, artifact, support-change,
human-intervention, and blocker hints ready for
`POST /fabrication/learning/outcomes`. Machine-ready release remains blocked while
orientation, support removal, postprocess access, split/combine, intervention,
artifact, or human dispositions are unresolved. Learning observations such as
`support-strategy-family:*`, `support-orientation:*`, `support-removal:*`,
`support-split-combine:*`, `support-strategy:support-change-required`, and
`support-strategy:split-combine-required` feed the bounded MDP/POMDP/neural
policy memory so future planners can reorient, change supports, split, combine,
reroute, or request human help before release.

## `GET /fabrication/process-recipes/catalog`

`GET /process-recipes/catalog` and the gateway-prefixed
`GET /fabrication/process-recipes/catalog` return the live
`dd.fabrication.process-recipe-catalog.v1` parameter and recipe evidence catalog
before generated or imported instructions are treated as ready for machine-code,
toolpath, simulation, postprocess, or release handoff. The catalog covers additive
slicer/extrusion profiles, subtractive feeds/speeds and cutter engagement, turning
threading and part-off parameters, sheet-cut cut charts and process media, and
thermal/chemical/finishing postprocess recipes.

Each process recipe family lists parameter evidence, release blockers, response
surfaces, and learning signals. The response names surfaces such as
`materialPlan.routeRequirements`, `toolingPlan.requirements`,
`controllerPlan.requiredControllerChecks`, `simulation.riskProfile`,
`qualityPlan.measurementTargets`, `postprocessPlan.requiredArtifacts`, and
`machineRelease.blockers`. Catalog entries are not certified production recipes.
Machine-ready release remains blocked until recipe
provenance, material/tool/machine compatibility, simulation, first-article or
coupon evidence, and operator or automation signoff clear. Recipe choices,
parameter revisions, feed/speed outcomes, thermal cycles, edge quality,
first-layer behavior, and postprocess results are retained as MDP/POMDP/neural
learning signals for future planning and instruction repair.

## `POST /fabrication/process-recipes/result`

`POST /process-recipes/result` and the gateway-prefixed
`POST /fabrication/process-recipes/result` accept retained
`dd.fabrication.process-recipe-result-review.v1` reviews from slicer, CAM,
postprocessor, simulation, coupon, or operator workers after selected recipe
parameters are checked against the material, tool, machine, controller, and
postprocess route.

The request records recipe reviews, parameter checks, first-article or coupon
results, artifacts, warnings, and learning observations. Responses retain
`process-recipe-result`, `process-recipe-reviews`,
`process-recipe-parameter-checks`, `process-recipe-coupon-results`,
`process-recipe-artifacts`, and `process-recipe-learning-observations` artifacts
on the fabrication job. The learning section includes a
`dd.fabrication.process-recipe-learning-outcome-draft.v1` `outcomeDraft` with
recipe, parameter, coupon, artifact, parameter-change, human-signoff, and blocker
hints ready for `POST /fabrication/learning/outcomes`. Machine-ready release
remains blocked while recipe provenance, feed/speed, thermal, cut-chart, support
media, coupon, artifact, or human signoff evidence is unresolved. Learning
observations such as
`process-recipe-family:*`, `process-recipe-kind:*`,
`process-recipe-parameter:*`, `process-recipe-coupon:*`,
`process-recipe:parameter-change-required`, and
`process-recipe:human-signoff-required` feed the bounded MDP/POMDP/neural policy
memory so future planners can choose safer parameters, revise recipes, request
coupons, or block risky generated/imported instructions before release.

## `GET /fabrication/kinematics/catalog`

`GET /kinematics/catalog` and the gateway-prefixed
`GET /fabrication/kinematics/catalog` return the live
`dd.fabrication.kinematics-catalog.v1` axis, coordinate-state, and motion
evidence catalog before generated or imported instructions are treated as ready
for simulation, postprocessing, or machine release. The catalog covers Cartesian
additive, router, and sheet motion; rotary/five-axis milling; turning,
mill-turn, and Swiss kinematics; robotic additive and external-axis cells; and
inspection/probing coordinate-state review.

Each kinematic family lists axes, required evidence, release blockers, response
surfaces, and learning signals. The response names surfaces such as
`simulation.axisExtents`, `simulation.riskProfile.programRisks`,
`controllerPlan.requiredControllerChecks`, `fixturePlan.setups.requiredEvidence`,
`monitoringPlan.monitorPoints`, `releaseProbePlan.probes`, and
`machineRelease.blockers`. Catalog entries are not certified kinematic
calibration records. Machine-ready release remains blocked until homing, units,
coordinate state, axis envelope, rotary/robot frames, fixture clearance,
simulation, and operator or automation signoff evidence clear. Axis-envelope,
coordinate-mode, TCP/frame, external-axis, spindle-sync, and clearance
observations are retained as MDP/POMDP/neural learning signals for future
planning and instruction repair.

## `POST /fabrication/kinematics/result`

`POST /kinematics/result` and the gateway-prefixed
`POST /fabrication/kinematics/result` accept retained
`dd.fabrication.kinematics-result-review.v1` reviews from simulation,
postprocessor, controller, probing, robotic-cell, or operator workers after
axis envelopes, coordinate state, TCP/frame calibration, rotary sync, spindle
sync, robot frames, or probe datum evidence are checked.

The request records axis checks, coordinate reviews, frame checks, artifacts,
warnings, and learning observations. Responses retain `kinematics-result`,
`kinematics-axis-checks`, `kinematics-coordinate-reviews`,
`kinematics-frame-checks`, `kinematics-artifacts`, and
`kinematics-learning-observations` artifacts on the fabrication job.
The learning section includes a
`dd.fabrication.kinematics-learning-outcome-draft.v1` `outcomeDraft` with axis,
coordinate-state, frame, artifact, human-intervention, and blocker hints ready
for `POST /fabrication/learning/outcomes`.
Machine-ready release remains blocked while axis envelope, homing, modal state,
work offsets, TCP/frame, rotary/robot sync, probe calibration, artifact, or human
intervention evidence is unresolved. Learning observations such as
`kinematics-family:*`, `kinematics-axis:*`, `kinematics-coordinate:*`,
`kinematics-frame:*`, and `kinematics:human-intervention-required` feed the
bounded MDP/POMDP/neural policy memory so future planners can choose safer
motions, insert probe/re-home stops, correct frames, or block risky generated
and imported instructions before release.

## `GET /fabrication/tolerances/catalog`

`GET /tolerances/catalog` and the gateway-prefixed
`GET /fabrication/tolerances/catalog` return the live
`dd.fabrication.tolerance-catalog.v1` dimensional, fit, GD&T/PMI, kerf, and
interface-control evidence catalog before generated or imported instructions are
treated as ready for quality planning, split/combine assembly, or machine
release. The catalog covers additive shrinkage and printed fits, subtractive
GD&T feature controls, turning/threading fits, sheet-cut kerf and edge quality,
and hybrid assembly interface stackups.

Each tolerance family lists geometry scopes, required evidence, release
blockers, response surfaces, and learning signals. The response names surfaces
such as `designInputReview.pmi`, `materialPlan.routeRequirements`,
`slicerPlan.profileEvidence`, `fixturePlan.datumTransfers`,
`decompositionPlan.parts`, `interfaceControlPlan.interfaces`,
`assemblyPlan.requiredEvidence`, `qualityPlan.measurementTargets`, and
`machineRelease.blockers`. Catalog entries are not certified inspection plans.
Machine-ready release remains blocked until tolerance-critical features have
material/process allowance, datum, metrology, inspection, and operator or
automation signoff evidence. Coupon measurements, first-article results, gauge
outcomes, kerf offsets, fit-up interventions, and split/combine stackups are
retained as MDP/POMDP/neural learning signals for future planning and
instruction repair.

## `POST /fabrication/tolerances/result`

`POST /tolerances/result` and the gateway-prefixed
`POST /fabrication/tolerances/result` accept retained tolerance-review outcomes
from dimensional, GD&T/PMI, fit, kerf, compensation, and interface-control
workers. The response uses `dd.fabrication.tolerance-result-review.v1` and
captures tolerance checks, fit/interface checks, compensation actions, retained
artifacts, release blockers, warning counts, and MDP/POMDP/neural learning
observations.

Machine-ready and release-ready status remains blocked when tolerance-critical
features are out of tolerance, interface fits are rejected, compensation has not
been applied, retained artifact evidence is missing, or the job still requires
redesign, rework, split/combine planning, or human fit-up. The result is stored
with `tolerance-result`, `tolerance-checks`, `tolerance-fit-checks`,
`tolerance-compensations`, `tolerance-artifacts`, and
`tolerance-learning-observations` artifacts so future planners can learn which
allowances, offsets, interface designs, or human checkpoints made generated or
imported instructions releasable.
The `learning.outcomeDraft` uses
`dd.fabrication.tolerance-learning-outcome-draft.v1` and carries
tolerance-family, geometry-scope, fit, compensation, artifact, blocker,
split/combine, human-intervention, reward, and submit-route hints for
`POST /fabrication/learning/outcomes`.

## `GET /fabrication/process-capabilities/catalog`

`GET /process-capabilities/catalog` and the gateway-prefixed
`GET /fabrication/process-capabilities/catalog` return the live
`dd.fabrication.process-capability-catalog.v1` geometry/process capability
catalog before generated, imported, or improved instructions are treated as
ready for machine release. The catalog covers additive printability envelopes,
subtractive tool access and chip-load envelopes, turning workholding and
part-off envelopes, sheet-cut kerf/pierce/support envelopes, and hybrid
split/combine and rework envelopes.

Each capability family lists capability scopes, required evidence, failure
boundaries, response surfaces, and learning signals. The response names
surfaces such as `designInputReview.capabilityFindings`,
`slicerPlan.profileEvidence`, `toolingPlan.requirements`,
`processRecipe.cutChart`, `decompositionPlan.parts`,
`interfaceControlPlan.interfaces`, `qualityPlan.measurementTargets`, and
`machineRelease.blockers`. Catalog entries are not certified machine capability
studies. Machine-ready release remains blocked when requested geometry exceeds
reviewed process capability and no redesign, alternate route, split/combine
plan, or human-intervention evidence is present. Capability failures, alternate
routes, split boundaries, and measured process outcomes are retained as
MDP/POMDP/neural learning signals for future planning and instruction repair.

## `POST /fabrication/process-capabilities/result`

`POST /process-capabilities/result` and the gateway-prefixed
`POST /fabrication/process-capabilities/result` accept retained process
capability outcomes from printability, tool-access, workholding, kerf/pierce,
turning, and hybrid split/combine reviewers. The response uses
`dd.fabrication.process-capability-result-review.v1` and captures capability
findings, alternate route decisions, measured coupon or first-article results,
retained artifacts, release blockers, warning counts, a `priorityDispositions`
array for machine-failure, human-intervention, split/combine or interface,
non-G-code/job-sheet evidence, and learning-feedback lanes, plus MDP/POMDP/neural
learning observations. The learning section also emits a
`dd.fabrication.process-capability-learning-outcome-draft.v1` `outcomeDraft`
with capability-family, capability-scope, alternate-route, measurement,
artifact, priority-disposition, split/combine, redesign, human-intervention,
blocker, reward, and submit-route hints for `POST /fabrication/learning/outcomes`.

Machine-ready and release-ready status remains blocked when requested geometry
exceeds a reviewed process envelope, alternate routes are not accepted,
measurements are out of limits, retained artifact evidence is missing, or the
job still requires redesign, split/combine planning, or human intervention. The
result is stored with `process-capability-result`,
`process-capability-findings`, `process-capability-alternate-routes`,
`process-capability-measurements`, `process-capability-artifacts`,
`process-capability-priority-dispositions`, and
`process-capability-learning-observations` artifacts so future planners can
learn which printer, mill, lathe, sheet-cut, or hybrid routes actually made the
requested part releasable. Capability observations include
`process-capability-priority:<priority>:<disposition>` signals.

## `GET /fabrication/manufacturability/catalog`

`GET /manufacturability/catalog` and the gateway-prefixed
`GET /fabrication/manufacturability/catalog` return the live
`dd.fabrication.manufacturability-catalog.v1` design-for-manufacture review
catalog before generated, imported, or improved instructions are treated as
ready for machine-code generation or release. The catalog covers CAD topology and
design-intent review, additive DFM print-or-split review, subtractive and
turning DFM access review, sheet-cut and flat-pattern review, and hybrid
assembly DFM/interface review.

Each manufacturability review family lists source kinds, machine kinds, check
scopes, required evidence, failure boundaries, response surfaces, and learning
signals. The response names surfaces such as
`designInputReview.manufacturabilityEvidence`,
`designInputReview.conversionPlan`, `processCapabilityContracts`,
`decompositionPlan.parts`, `interfaceControlPlan.interfaces`,
`assemblyPlan.requiredEvidence`, `qualityPlan.measurementTargets`, and
`machineRelease.blockers`. Catalog entries are not certified design approvals.
Machine-ready release remains blocked when CAD, mesh, sheet, or assembly
geometry needs redesign, repair, alternate routing, split/combine planning, or
human-intervention evidence. Manufacturability failures, redesign actions,
split/combine decisions, and successful route outcomes are retained as
MDP/POMDP/neural learning signals for future planning and instruction repair.

## `POST /fabrication/manufacturability/result`

`POST /manufacturability/result` and the gateway-prefixed
`POST /fabrication/manufacturability/result` let DFM/DfAM, tool-access,
support-access, flat-pattern, workholding, and hybrid-interface reviewers report
retained manufacturability evidence back to the fabrication server. The
`dd.fabrication.manufacturability-result-review.v1` response reviews findings,
route feasibility, split/combine or redesign decisions, retained artifacts,
human-intervention requirements, and learning observations, then stores
`manufacturability-result`, `manufacturability-findings`,
`manufacturability-route-reviews`, `manufacturability-split-combine-decisions`,
`manufacturability-artifacts`, and `manufacturability-learning-observations`
artifacts.

Machine-ready release remains blocked when manufacturability workers fail,
findings are missing or unresolved, geometry requires redesign or
split/combine, route reviews are infeasible, artifacts lack URI/checksum
evidence, or human intervention is still required. Observations such as
`manufacturability-family:*`, `manufacturability-scope:*`,
`manufacturability-route:*`, `manufacturability-decision:*`,
`manufacturability:redesign-required`, and
`manufacturability:split-combine-required` feed the bounded MDP/POMDP/neural
policy memory so future planners can redesign, split, combine, reroute, or
request human review before machine-code generation.
The `learning.outcomeDraft` uses
`dd.fabrication.manufacturability-learning-outcome-draft.v1` and carries
review-family, check-scope, route, split/combine decision, artifact, blocker,
redesign, human-intervention, reward, and submit-route hints for
`POST /fabrication/learning/outcomes`.

## `GET /fabrication/failure-modes/catalog`

`GET /failure-modes/catalog` and the gateway-prefixed
`GET /fabrication/failure-modes/catalog` return the live
`dd.fabrication.failure-mode-catalog.v1` process-failure catalog before
generated, imported, or improved instructions are treated as ready for
simulation, execution, or release. The catalog covers additive print-process
failures, subtractive tool and fixture failures, turning and part-transfer
failures, sheet-cutting utility and slug failures, and hybrid route or
human-intervention failures.

Each failure-mode family lists machine kinds, failure modes, early signals,
required evidence, release blockers, remediation routes, response surfaces, and
learning signals. The response names surfaces such as
`boundarySummary.boundaries`, `interventionMap.requiredInterventions`,
`simulation.riskProfile.programRisks`, `decompositionPlan.parts`,
`executionPlan.stopPoints`, `learning.outcomes`, and
`machineRelease.blockers`. Catalog entries are not certified machine
diagnostics. Machine-ready release remains blocked while likely failure modes
require unresolved human intervention, redesign, support restart, tool/process
state recovery, or split/combine planning. Failure signatures, remediation
choices, split/combine outcomes, and operator interventions are retained as
MDP/POMDP/neural learning signals for future planning and instruction repair.

## `POST /fabrication/failure-modes/result`

`POST /failure-modes/result` and the gateway-prefixed
`POST /fabrication/failure-modes/result` accept retained
`dd.fabrication.failure-mode-result-review.v1` reviews from process monitors,
simulators, import validators, or operators after a generated, imported, or
improved toolpath is checked against likely failure modes.

The request records failure events, recovery actions, human interventions,
artifacts, split/combine needs, and learning observations. Responses include a
`priorityDispositions` array for machine-failure, human-intervention,
split/combine or interface review, non-G-code/job-sheet evidence, and learning
feedback lanes. Responses store `failure-mode-result`, `failure-mode-events`,
`failure-mode-recovery-actions`, `failure-mode-interventions`,
`failure-mode-artifacts`, `failure-mode-priority-dispositions`, and
`failure-mode-learning-observations` artifacts on the retained fabrication job.
Machine-ready release remains blocked while a failure event, recovery action,
intervention, artifact, human disposition, or split/combine requirement is still
unresolved. Learning observations such as `failure-family:*`,
`failure-mode:*`, `failure-recovery:*`, `failure-intervention:*`,
`failure-mode:support-failure`, `failure-mode:split-combine-required`, and
`failure-mode-priority:<priority>:<disposition>` feed the bounded
MDP/POMDP/neural policy memory so future planners can reroute, recover, split,
combine, or request human help before release.
The `learning.outcomeDraft` uses
`dd.fabrication.failure-mode-learning-outcome-draft.v1` and carries
failure-family, failure-mode, recovery-action, intervention, artifact, blocker,
priority-disposition, split/combine, human-intervention, reward, and
submit-route hints for `POST /fabrication/learning/outcomes`.

## `GET /fabrication/safety/catalog`

`GET /safety/catalog` and the gateway-prefixed
`GET /fabrication/safety/catalog` return the live
`dd.fabrication.safety-catalog.v1` guarding, interlock, extraction, emergency,
and human-intervention evidence catalog before generated or imported
instructions are treated as ready for unattended or machine-ready release. The
catalog covers additive enclosure and thermal safety, CNC guarding and chip
control, sheet-cutting energy and extraction safety, robotic-cell and
external-axis interlocks, and release lockout/emergency response.

Each safety family lists hazards, required evidence, release blockers, response
surfaces, and learning signals. The response names surfaces such as
`executionPlan.stopPoints`, `executionPlan.operatorActions`,
`interventionMap.requiredInterventions`, `monitoringPlan.monitorPoints`,
`monitoringPlan.alertRules`, `monitoringPlan.recoveryActions`,
`releasePackagePlan.requiredArtifacts`, and `machineRelease.blockers`. Catalog
entries are not certified machine-safety approvals. Machine-ready release
remains blocked until machine guarding, process support, operator intervention,
emergency response, monitoring, alerting, and release signoff evidence clear.
Interlock states, operator stops, extraction failures, E-stop events, recovery
actions, and unattended-release outcomes are retained as MDP/POMDP/neural
learning signals for future planning and instruction repair.

## `POST /fabrication/safety/result`

`POST /safety/result` and the gateway-prefixed
`POST /fabrication/safety/result` accept retained safety outcomes from guarding,
interlock, extraction, robot-cell, emergency-stop, restart, and human
intervention reviewers. The response uses
`dd.fabrication.safety-result-review.v1` and captures safety checks, interlock
checks, emergency actions, retained artifacts, release blockers, warning counts,
and MDP/POMDP/neural learning observations. The learning section includes a
`dd.fabrication.safety-learning-outcome-draft.v1` `outcomeDraft` with safety
family, hazard, interlock, emergency-action, artifact, stop-point, restart-review,
and human-intervention hints ready for `POST /fabrication/learning/outcomes`.

Machine-ready and release-ready status remains blocked when hazards are not
cleared, interlocks are not verified, stop points or restart reviews remain
open, emergency actions are incomplete, retained artifact evidence is missing,
or human intervention is still required. The result is stored with
`safety-result`, `safety-checks`, `safety-interlock-checks`,
`safety-emergency-actions`, `safety-artifacts`, and
`safety-learning-observations` artifacts so future planners can learn where
generated/imported instructions need safe stops, interlock verification,
operator handoff, or earlier rejection.

## `GET /fabrication/environment/catalog`

`GET /environment/catalog` and the gateway-prefixed
`GET /fabrication/environment/catalog` return the live
`dd.fabrication.environment-catalog.v1` humidity, thermal, utility,
extraction, vibration, and metrology-environment evidence catalog before
generated or imported instructions are treated as ready for material planning,
monitoring, inspection, or machine release. The catalog covers additive material
storage and printroom state, subtractive coolant/chip and thermal stability,
sheet-cutting extraction and utility state, robotic-cell space and utility
readiness, and inspection/metrology release environment.

Each environment family lists condition scopes, required evidence, release
blockers, response surfaces, and learning signals. The response names surfaces
such as `materialPlan.routeRequirements`, `processRecipe.materialConditioning`,
`processRecipe.coolant`, `monitoringPlan.monitorPoints`,
`monitoringPlan.alertRules`, `qualityPlan.measurementTargets`,
`calibrationPlan.requiredEvidence`, `releasePackagePlan.requiredArtifacts`, and
`machineRelease.blockers`. Catalog entries are not certified facility
qualifications. Machine-ready release remains blocked until material
conditioning, ambient/process utilities, extraction, thermal stability,
monitoring, inspection environment, and signoff evidence clear. Humidity,
drying, coolant, extraction, utility, vibration, temperature, and metrology
outcomes are retained as MDP/POMDP/neural learning signals for future planning
and instruction repair.

## `POST /fabrication/environment/result`

`POST /environment/result` and the gateway-prefixed
`POST /fabrication/environment/result` accept retained environment outcomes from
material-conditioning, ambient/chamber, coolant, extraction, utility, vibration,
and metrology-environment reviewers. The response uses
`dd.fabrication.environment-result-review.v1` and captures condition checks,
utility checks, metrology checks, retained artifacts, release blockers, warning
counts, and MDP/POMDP/neural learning observations. The learning section includes
a `dd.fabrication.environment-learning-outcome-draft.v1` `outcomeDraft` with
environment-family, condition-scope, utility, metrology, artifact, recovery,
recheck, and human-intervention hints ready for
`POST /fabrication/learning/outcomes`.

Machine-ready and release-ready status remains blocked when environmental
conditions are out of limits, drying or conditioning is incomplete, extraction or
utilities require recovery, metrology conditions are unstable, retained artifact
evidence is missing, or human intervention is still required. The result is
stored with `environment-result`, `environment-condition-checks`,
`environment-utility-checks`, `environment-metrology-checks`,
`environment-artifacts`, and `environment-learning-observations` artifacts so
future planners can learn which material, machine, utility, inspection, and
ambient conditions made generated/imported instructions releasable.

## `GET /fabrication/provenance/catalog`

`GET /provenance/catalog` and the gateway-prefixed
`GET /fabrication/provenance/catalog` return the live
`dd.fabrication.provenance-catalog.v1` traceability catalog before generated,
imported, improved, or learned fabrication instructions are treated as ready for
machine release. The catalog covers design-input and CAD lineage, material lots
and feedstock traceability, machine-program and controller artifact lineage,
inspection/release/nonconformance ledgers, and learning-outcome/policy-memory
lineage.

Each provenance family lists evidence scopes, required evidence, release
blockers, response surfaces, and learning signals. The response names surfaces
such as `designInputReview.conversionPlan`, `designPackage.parts`,
`materialPlan.routeRequirements`, `machineCodePackage.programs`,
`qualityPlan.measurementTargets`, `releasePackagePlan.packages`,
`learning.policySnapshot`, `learning.outcomes`, and `machineRelease.blockers`.
Catalog entries are not certified quality records. Machine-ready release
remains blocked until source artifacts, material lots, generated or imported
programs, inspection results, release bundles, and learning outcomes have
traceable hashes, revisions, review status, and signoff evidence. Artifact
hashes, conversion logs, lot records, controller program digests, inspection
dispositions, nonconformance decisions, and learning outcome lineage are
retained as MDP/POMDP/neural learning signals for future planning and
instruction repair.

## `GET /fabrication/as-built/catalog`

`GET /as-built/catalog` and the gateway-prefixed
`GET /fabrication/as-built/catalog` return the live
`dd.fabrication.as-built-catalog.v1` actual-geometry evidence catalog before
scan, CMM, deviation-map, or interface-fit observations are treated as release
evidence. The catalog covers scan-to-design deviation maps, additive layer and
defect evidence, subtractive feature and datum evidence, and hybrid
split/combine as-built interface evidence.

Each as-built family lists measured-geometry evidence scopes, required
evidence, release blockers, response surfaces, and learning signals. The
response names surfaces such as `qualityResult.measurements`,
`toolpathResult.simulationChecks`, `releasePackagePlan.requiredArtifacts`,
`machineRelease.blockers`, `decompositionPlan.parts`,
`interfaceControlPlan.controls`, `handoffResult.evidence`, and
`learning.outcomes`, plus artifacts such as `as-built-deviation-map`,
`as-built-scan-mesh`, `as-built-cmm-report`, and
`as-built-interface-fit-record`. Catalog entries are not certified metrology
acceptance. Machine-ready release remains blocked while scan/CMM evidence,
deviation-map artifacts, datum alignment, interface-fit proof, or as-built
lineage is missing or unresolved. Actual geometry and split/combine interface
observations are retained as MDP/POMDP/neural learning signals so future
planning can learn when to add inspection, split parts, change machines,
reroute features, or require human signoff.

## `POST /fabrication/as-built/result`

`POST /as-built/result` and the gateway-prefixed
`POST /fabrication/as-built/result` accept retained as-built outcomes from
scan-to-design, CMM, probing, additive layer-inspection, subtractive feature
inspection, and split/combine interface-fit reviewers. The response uses
`dd.fabrication.as-built-result-review.v1` and captures measurement checks,
deviation maps, interface checks, retained artifacts, release blockers, warning
counts, MDP/POMDP/neural learning observations, and a
`dd.fabrication.as-built-learning-outcome-draft.v1` payload that callers can send
to `POST /fabrication/learning/outcomes`.

Machine-ready and release-ready status remains blocked when measured actual
geometry is missing, deviation maps are unaligned or out of tolerance,
split/combine interfaces lack fit or datum-transfer proof, as-built artifacts
lack URI/checksum/evidence labels, remeasurement or rework is required, or human
intervention is still open. The result is stored with `as-built-result`,
`as-built-measurement-checks`, `as-built-deviation-maps`,
`as-built-interface-checks`, `as-built-artifacts`, and
`as-built-learning-observations` artifacts so future planners can learn which
scan, CMM, probe, interface-fit, and actual-geometry evidence made generated,
imported, split, or combined fabrication instructions releasable.

## `POST /fabrication/provenance/result`

`POST /provenance/result` and the gateway-prefixed
`POST /fabrication/provenance/result` accept retained provenance outcomes from
CAD translators, material-lot reviewers, machine-code/postprocessor reviewers,
inspection ledgers, release-bundle builders, and learning-policy lineage checks.
The response uses `dd.fabrication.provenance-result-review.v1` and captures
lineage checks, artifact digest checks, custody/signoff events, retained
artifacts, release blockers, warning counts, and MDP/POMDP/neural learning
observations. The learning section includes a
`dd.fabrication.provenance-learning-outcome-draft.v1` `outcomeDraft` with
provenance-family, evidence-scope, artifact-kind, custody-event, blocker,
review, and human-intervention hints ready for
`POST /fabrication/learning/outcomes`.

Machine-ready and release-ready status remains blocked when design, material,
machine-program, inspection, release-package, or learning lineage is missing,
untraceable, mismatched, or still awaiting human review. The result is stored
with `provenance-result`, `provenance-lineage-checks`,
`provenance-artifact-checks`, `provenance-custody-events`,
`provenance-artifacts`, and `provenance-learning-observations` artifacts so
future planners can learn which CAD/source, material, controller-program,
inspection, signoff, and release-package evidence made generated or imported
instructions releasable.

## `POST /fabrication/setup/plan`

`POST /setup/plan` and the gateway-prefixed
`POST /fabrication/setup/plan` accept the same request body as
`POST /fabrication/plan`, apply bounded learning-policy memory, run the planner,
store and publish the full plan result, and return a compact
`dd.fabrication.setup-planning.v1` setup-readiness package. The response focuses
on `toolingPlan.requirements`, `toolingPlan.requirements.requiredTools`,
`toolingPlan.requirements.workholding`, `toolingPlan.requirements.setupChecks`,
`fixturePlan.setups`, `fixturePlan.setups.datumScheme`,
`fixturePlan.setups.requiredEvidence`, `fixturePlan.setups.clearanceChecks`,
`fixturePlan.datumTransfers`, `monitoringPlan.monitorPoints`,
`monitoringPlan.monitorPoints.channels`, `monitoringPlan.alertRules`,
`monitoringPlan.recoveryActions`, `machineRelease.blockers`,
`releasePackagePlan.requiredArtifacts`, and `learning.releaseProbePlan`.

Setup plans are draft tooling, fixture, datum, workholding, and monitoring
evidence packages, not certified fixture designs or machine-safety approvals.
Machine-ready release remains blocked while tooling blockers, fixture evidence
gaps, datum-transfer gates, monitoring channels, recovery actions, or signoff
gates are unresolved. Stored artifacts include `tooling-plan`, `fixture-plan`,
`monitoring-plan`, `machine-release`, `release-package-plan`,
`simulation-report`, and `mdp-request` so MDP/POMDP/neural workers can learn
when to change workholding, split setups, add automation, regenerate
instructions, or require human intervention.

## `POST /fabrication/setup/result`

`POST /setup/result` and the gateway-prefixed
`POST /fabrication/setup/result` accept setup worker results, normalize them into
`dd.fabrication.setup-result-review.v1`, and store a bounded review job with
retained setup-check, datum-transfer, monitoring-channel, artifact, and
learning-observation surfaces plus a
`dd.fabrication.setup-learning-outcome-draft.v1` payload that callers can send to
`POST /fabrication/learning/outcomes`. The response reports blocker counts for
failed or human-intervention setup checks, out-of-tolerance datum transfers,
monitoring channels without heartbeat or safe-stop evidence, restart blockers,
and missing setup artifact evidence.

Setup result reviews are retained tooling, fixture, datum, workholding, and
monitoring evidence, not certified fixture design or a machine-safety waiver.
Machine-ready and unattended release remain blocked until setup checks, datum
transfers, monitoring channels, retained artifacts, and human dispositions
clear. Stored artifacts include `setup-result`, `setup-checks`,
`setup-datum-transfers`, `setup-monitoring-channels`, `setup-artifacts`, and
`setup-learning-observations` so MDP/POMDP/neural workers can learn when to
change workholding, split setups, add automation, regenerate instructions, or
require human signoff before release.

## `GET /fabrication/monitoring/catalog`

`GET /monitoring/catalog` and the gateway-prefixed
`GET /fabrication/monitoring/catalog` return the live
`dd.fabrication.monitoring-catalog.v1` runtime monitoring, safe-stop, recovery,
and restart-authority catalog before callers treat generated or imported work as
machine-ready or unattended. The payload exposes monitoring contracts for
additive printers, mill/router cells, turning and mill-turn/Swiss workholding,
sheet-cut process-media/fire/fume/wire/slug monitoring, assembly-cell
vision/force/interlock monitoring, and unattended recovery governance. It names
response surfaces such as `monitoringPlan.monitorPoints`,
`monitoringPlan.alertRules`, `monitoringPlan.recoveryActions`,
`monitoringPlan.releaseGates`, `machineRelease.blockers`,
`operatorInterventionPlan.requiredOperatorActions`,
`validation.failureBoundaries`, `releaseProbePlan.probes`,
`monitoringResult.channels`, `monitoringResult.alerts`,
`monitoringResult.recoveryActions`, and
`monitoringResult.operatorInterventions`, plus retained artifact surfaces
`monitoring-plan`, `monitoring-result`, `monitoring-alerts`,
`monitoring-recovery-actions`, `monitoring-operator-interventions`,
`parametric-design.monitoringPlan`, and matching `mdp-request` monitoring
artifacts. Catalog entries are runtime
evidence contracts, not certified safety systems or controller restart
procedures; machine-ready and unattended release remain blocked while monitor
channels, alert rules, safe-stop behavior, recovery actions, or restart
authority are unresolved. Monitoring and recovery observations are retained for
MDP/POMDP/neural workers so future planning can learn when to add sensors, split
jobs, require operators, or improve generated instructions.

## `POST /fabrication/monitoring/plan`

`POST /monitoring/plan` and the gateway-prefixed
`POST /fabrication/monitoring/plan` accept the same request body as
`POST /fabrication/plan`, apply bounded learning-policy memory, run the planner,
store and publish the full plan result, and return a compact
`dd.fabrication.monitoring-planning.v1` runtime monitoring and recovery package.
The response focuses on `monitoringPlan.monitorPoints`,
`monitoringPlan.monitorPoints.channels`,
`monitoringPlan.monitorPoints.expectedSignals`,
`monitoringPlan.monitorPoints.requiredEvidence`,
`monitoringPlan.monitorPoints.recoveryActions`, `monitoringPlan.alertRules`,
`monitoringPlan.alertRules.automatedResponse`,
`monitoringPlan.recoveryActions`, `monitoringPlan.releaseGates`,
`executionPlan.stopPoints`, `operatorInterventionPlan.requiredOperatorActions`,
`machineRelease.blockers`, `learning.releaseProbePlan`, and result handoff
routes for `POST /monitoring/result` and
`POST /fabrication/monitoring/result`.

Monitoring plans are draft runtime channel, alert, safe-stop, recovery, and
restart-authority evidence packages, not certified safety systems or controller
restart procedures. Machine-ready and unattended release remain blocked while
monitor channels, alert rules, recovery actions, operator check-in, or restart
authority are unresolved. Stored artifacts include `monitoring-plan`,
`operator-intervention-plan`, `execution-plan`, `machine-release`,
`simulation-report`, `monitoring-result`, `monitoring-alerts`,
`monitoring-recovery-actions`, `monitoring-operator-interventions`, and
`mdp-request` so MDP/POMDP/neural workers can learn when to add sensors, split
jobs, require operators, add automation, or improve generated instructions.

## `POST /fabrication/monitoring/result`

`POST /monitoring/result` and the gateway-prefixed
`POST /fabrication/monitoring/result` accept runtime monitoring worker results,
normalize them into `dd.fabrication.monitoring-result-review.v1`, and store a
bounded review job with retained channel, alert, recovery-action,
operator-intervention, artifact, and learning-observation surfaces plus a
`dd.fabrication.monitoring-learning-outcome-draft.v1` payload that callers can
send to `POST /fabrication/learning/outcomes`. The response reports blocker
counts for missing channel heartbeat or signal-envelope evidence, unresolved
critical alerts, safe-stop triggers, restart blockers, operator interventions,
and missing monitoring artifact evidence.

Monitoring result reviews are retained runtime telemetry, alert, safe-stop,
recovery, restart-authority, and operator-check-in evidence, not certified safety
systems or controller restart procedures. Machine-ready and unattended release
remain blocked until channels, alerts, recovery actions, retained artifacts, and
human dispositions clear. Stored artifacts include `monitoring-result`,
`monitoring-alerts`, `monitoring-channels`,
`monitoring-recovery-actions`, `monitoring-operator-interventions`,
`monitoring-artifacts`, and `monitoring-learning-observations` so
MDP/POMDP/neural workers can learn when to add sensors, split jobs, require
operators, change recovery actions, or improve generated instructions before
release.

## `GET /fabrication/postprocess/catalog`

`GET /postprocess/catalog` and the gateway-prefixed
`GET /fabrication/postprocess/catalog` return the live
`dd.fabrication.postprocess-catalog.v1` finishing, traveler, controller-output,
and release-evidence catalog before callers treat generated or imported work as
machine-ready. The payload exposes target contracts for FDM support removal,
resin wash/cure, powder-bed cooldown and depowdering, metal additive stress
relief and plate removal, binder-jet cure/sinter or infiltration, subtractive
deburr/clean/protect work, sheet-cut edge cleanup and slug release, and
assembly/join cure plus final-fit checks. It also exposes artifact contracts for
inspection report packaging with calibration records, datum alignment and
uncertainty records, first-article measured values, and nonconformance
disposition records, thermal profile and furnace logs, fixture/setter and
atmosphere records, cooldown/quench and PPE records,
distortion/hardness/release inspection records, surface media/chemistry and SDS
records, masking/plugging/protected-feature records, ventilation/PPE/waste
records, finish thickness/adhesion/inspection records, welding procedure and
qualification records, joint fit-up/fixture/clamp records, filler/flux/gas and
fume-control records, heat-input/interpass/distortion records,
weld-inspection/NDE/repair records, mold master/tooling/release records,
mix-ratio/pot-life/batch records, degas/vacuum/pressure/cure records,
demold/shrinkage/void/dimensional records, flat-pattern and
bend-allowance records, press-brake tooling and tonnage records,
backgauge/bend-sequence/angle-inspection records, formed-part dimensional
release records, foam blank density/template records, wire
temperature/tension/kerf records, fume/fire-watch/PPE records, foam core
surface/taper/dimensional records, gear drawing/blank datum records, gear
cutter/arbor/indexing records, gear deburr/burr-control records, gear
over-pins/span/profile inspection records, Swiss guide-bushing/bar-feed
records, gang-tool/live-tool clearance records,
pickoff/cutoff/ejection records, and first-article runout/remnant records,
`postprocess-plan`, `analysis-postprocess-plan`, controller output packages, and
postprocess travelers, plus planning, result, and instruction-analysis route
aliases, response surfaces such as `postprocessPlan.status`,
`postprocessPlan.controllerTargets`, `postprocessPlan.requiredArtifacts`,
`postprocessPlan.blockers`, `postprocessResult.targets`,
`postprocessResult.gates`, `postprocessResult.travelerSteps`,
`postprocessResult.signoffs`, `postprocessResult.artifacts`,
`qualityPlan.releaseGates`,
`materialPlan.conditioning`, `releasePackagePlan.packages`, and
`machineRelease.blockers`. Catalog entries are evidence contracts, not certified
process completion; machine-ready release remains blocked while postprocess
targets, required artifacts, dry-run gates, quality checks, material
conditioning, or operator/automation signoff are unresolved. Postprocess
observations are retained for MDP/POMDP/neural workers so future planning can
learn when to add finishing operations, split parts, combine assemblies, or
require human intervention.

## `POST /fabrication/postprocess/plan`

`POST /postprocess/plan` and the gateway-prefixed
`POST /fabrication/postprocess/plan` accept the same request body as
`POST /fabrication/plan`, apply bounded learning-policy memory, run the planner,
store and publish the full plan result, and return a compact
`dd.fabrication.postprocess-planning.v1` controller-output and traveler
readiness package. The response focuses on `postprocessPlan.controllerTargets`,
`postprocessPlan.controllerTargets.gates`,
`postprocessPlan.controllerTargets.outputFormat`,
`postprocessPlan.controllerTargets.postprocessor`,
`postprocessPlan.requiredArtifacts`, `postprocessPlan.blockers`,
`postprocessPlan.learningObservations`, `controllerPlan.compatibilityTargets`,
`controllerPlan.releaseGates`, `releasePackagePlan.requiredArtifacts`,
`machineRelease.blockers`, `simulation.programs`, and result handoff routes for
`POST /postprocess/result` and `POST /fabrication/postprocess/result`.

Postprocess plans are draft controller-output, traveler, dry-run, and signoff
evidence packages, not certified machine-safety approvals or completed finishing
records. Machine-ready release remains blocked while controller targets,
postprocessors, dry-run gates, required artifacts, release packages, or
operator/automation signoff remain unresolved. Stored artifacts include
`postprocess-plan`, `controller-plan`, `release-package-plan`,
`simulation-report`, `machine-release`, generated and improved instruction
programs, `postprocess-result`, `postprocess-target-results`,
`postprocess-gates`, `postprocess-traveler-steps`, `postprocess-signoffs`, and
`mdp-request` so MDP/POMDP/neural workers can learn when to change
postprocessors, add finishing operations, split parts, combine assemblies, or
require human intervention.

## `POST /fabrication/postprocess/result`

`POST /postprocess/result` and the gateway-prefixed
`POST /fabrication/postprocess/result` accept postprocess worker results,
normalize them into `dd.fabrication.postprocess-result-review.v1`, and store a
bounded review job with retained target, gate, traveler-step, signoff, artifact,
and learning-observation surfaces. The response reports blocker counts for
controller target failures, unresolved dry-run or simulation gates, incomplete
traveler steps, missing operator/automation signoff, and missing postprocess
artifact evidence. The learning section includes a
`dd.fabrication.postprocess-learning-outcome-draft.v1` `outcomeDraft` with target
status, postprocessor, gate, traveler-step, signoff, artifact, blocker, traveler
completion, signoff approval, and human-intervention hints ready for
`POST /fabrication/learning/outcomes`.

Postprocess result reviews are retained controller-output, finishing, traveler,
dry-run, inspection, and signoff evidence, not certified process completion or
safety approval. Machine-ready and downstream release remain blocked until
target results, gates, traveler steps, signoffs, retained artifacts, and human
dispositions clear. Stored artifacts include `postprocess-result`,
`postprocess-target-results`, `postprocess-gates`,
`postprocess-traveler-steps`, `postprocess-signoffs`,
`postprocess-artifacts`, and `postprocess-learning-observations` so
MDP/POMDP/neural workers can learn when to change postprocessors, add finishing
operations, split parts, combine assemblies, improve instructions, or require
human signoff before release.
The `learning.outcomeDraft` uses
`dd.fabrication.postprocess-learning-outcome-draft.v1` and carries target-status,
postprocessor, gate, traveler-step, signoff, artifact, blocker,
human-intervention, reward, and submit-route hints for
`POST /fabrication/learning/outcomes`.

## `GET /fabrication/evidence/catalog`

`GET /evidence/catalog` and the gateway-prefixed
`GET /fabrication/evidence/catalog` return the
`dd.fabrication.evidence-catalog.v1` global evidence taxonomy for intake,
planning, generated or imported machine-code, setup, execution, release, and
learning review. It complements the retained artifact catalog by naming the
evidence families clients should attach before machine-ready release can be
considered: design-source evidence, machine/process evidence,
instruction/controller evidence, setup/execution evidence, quality/release
evidence, and learning-outcome evidence.

Each family names response surfaces such as `designInputReview`,
`machineSelection`, `controllerPlan`, `validation.failureBoundaries`,
`executionPlan.stopPoints`, `qualityPlan`, `releasePackagePlan`,
`machineRelease.blockers`, `learningPolicySnapshot`, and `neuralTrainingCorpus`.
Examples include native CAD source-system and translator evidence, machine
profile and material/feedstock state, controller modal-state and postprocessor
review, datum/probe/workholding records, inspection and traveler signoff,
reward signals, boundary resolution actions, split/combine results, and
human-intervention outcomes.

Evidence catalog entries are intake and review requirements, not certified
shop-floor approval. `machineReady` remains false until required design,
machine, instruction, setup, quality, release, and signoff evidence clear. DES,
MDP/POMDP, and neural workers can reprioritize future routes from evidence and
outcomes, but learned preferences stay advisory until release gates clear.

## `GET /fabrication/artifacts/catalog`

`GET /artifacts/catalog` and the gateway-prefixed
`GET /fabrication/artifacts/catalog` return the live
`dd.fabrication.artifact-catalog.v1` catalog for retained plan, instruction
analysis, release, and learning evidence before callers fetch individual job
artifacts. The payload groups artifact contracts for design/CAD handoff,
generated design exports, generated or imported machine instruction work,
release/execution evidence, setup/quality/monitoring evidence, split/combine and
assembly evidence, DES-backed MDP/POMDP/neural learning evidence, and outcome
learning evidence. Learning artifact surfaces include `learning-policy-snapshot`,
`learning-outcome-memory`, `learning-corpus`, `neural-training-corpus`,
`mdp-request`, `reward-signal`, `mdp-experience`, and `neural-example` so
policy, outcome-memory, and neural-corpus workers can discover retained evidence
before fetching a job artifact. It names retrieval routes such as `GET /jobs`,
`GET /fabrication/jobs`, `GET /jobs/:job_id`, `GET /fabrication/jobs/:job_id`,
`GET /jobs/:job_id/release-bundle`,
`GET /fabrication/jobs/:job_id/release-bundle`,
`GET /jobs/:job_id/artifacts/:artifact_id`, and
`GET /fabrication/jobs/:job_id/artifacts/:artifact_id`, plus surfaces including
`job.releaseBundle`, `releaseBundle.releaseSurfaces`,
`releaseBundle.artifacts`, `generatedPrograms`, `improvedPrograms`,
`designExports`, `releasePackagePlan`, `learning`, and artifact fields such as
`artifactId`, `kind`, `mediaType`, `draft`, `machineReady`, and `content`.
Catalog entries describe bounded in-process evidence surfaces, not durable
database storage or certified machine release; generated design exports, machine programs, improved programs,
release packages, DES/POMDP/neural artifacts, and
learning outcomes remain draft evidence until validation, simulation,
controller, setup, quality, and signoff gates clear.

## `GET /fabrication/packages/catalog`

`GET /packages/catalog` and the gateway-prefixed
`GET /fabrication/packages/catalog` return the live
`dd.fabrication.package-catalog.v1` retained package contract for moving a
fabrication request from design/source evidence through machine/process,
instruction/controller, hybrid boundary, release, and learning-feedback
evidence. The catalog makes the intake package checklist directly discoverable
without fetching the broader intake guide.

Package phases name required artifacts such as `design-input-review`,
`design-package`, `generated-design-export`, `machine-selection`,
`process-plan`, `process-graph`, `machine-code-result`,
`instruction-validation-result`, `improved-program-*`,
`interface-control-plan`, `release-package-plan`, `mdp-request`,
`learning-policy-snapshot`, and `learning-outcome-memory`. They also link
source routes including `POST /fabrication/design/generate`,
`POST /fabrication/machine-code/generate`,
`POST /fabrication/instructions/validate`,
`POST /fabrication/instructions/improve`,
`POST /fabrication/decomposition/plan`, and
`POST /fabrication/release/preview`.

The catalog also exposes a `releaseHandoffMatrix` so release workers can inspect
the exact evidence boundary for generated design exports, generated machine code,
imported instruction streams, improved instruction patch manifests, hybrid
recomposition packages, and learning feedback. Matrix rows require immutable
original instruction retention, neutral export and program checksums,
controller/postprocessor and exact-program dry-run or simulation evidence,
post-patch validation, child-route artifacts for printed, milled, turned, cut,
or manual operations, interface controls, quality proof, MDP/POMDP/DES/neural
primitive provenance, and promotion blockers. Rows explicitly block release for
ambiguous CAD sources, unreviewed imported programs, unreviewed instruction
patches, unresolved interface controls, missing outcome artifact links, and
attempted release-gate bypasses.

Package catalog entries are evidence requirements, not certified manufacturing
approval. `machineReady` remains false until design, machine, process,
instruction, simulation, quality, operator, interface, release, and learning
feedback blockers are retained and cleared. MDP/POMDP/neural feedback can
reprioritize future packages, but it cannot bypass package release gates.

## `POST /fabrication/packages/plan`

`POST /packages/plan` and the gateway-prefixed
`POST /fabrication/packages/plan` accept the same `FabricationPlanRequest`
payload as `POST /fabrication/plan`, run the normal learned fabrication planner,
retain the job evidence, publish the same plan outputs, and return
`dd.fabrication.package-planning.v1`. The response projects the generated
fabrication plan into a `dd.fabrication.package-plan.v1` package plan with
design/source, machine/process, instruction/controller, hybrid boundary/release,
and learning-feedback phases.

Each phase carries required artifacts, response surfaces, next routes, and
blockers so a client can see whether design export, machine selection,
`process-graph`, `machine-code-result`, `instruction-validation-result`,
`interface-control-plan`, `release-package-plan`, `mdp-request`, or
`learning-outcome-memory` evidence is still missing before release review.
Package planning also returns counts for generated programs, machine-ready
programs, design exports, process nodes, split/combine decisions, human
interventions, validation boundaries, simulation boundaries, release blockers,
and neural/MDP/POMDP learning feedback.

Package plans are orchestration evidence, not certified manufacturing release.
Generated and improved instruction packages must preserve original program,
patch, validation, simulation, and boundary-review artifacts; learned
MDP/POMDP/neural feedback can reprioritize future package plans but cannot
bypass release gates.

## `GET /fabrication/jobs/catalog`

`GET /jobs/catalog` and the gateway-prefixed `GET /fabrication/jobs/catalog`
return the live `dd.fabrication.job-evidence-catalog.v1` discovery payload for
the bounded in-process job ledger before callers fetch specific jobs,
release-bundles, or artifacts. The payload reports `maxJobs`, current retained
job/artifact counts, current job kinds, retrieval routes, producer routes,
record/detail/release-bundle surfaces, artifact families, and learning surfaces
such as `learningPolicySnapshot`, `mdp-request`, `pomdp-belief-state`,
`release-probe-plan`, `neural-training-corpus`, and `outcome-learning-event`.
Catalog policy makes the boundary explicit: retained jobs are review evidence
for CAD/CAM, slicer, controller, setup, simulation, release, and learning
workers, not durable database storage or certified production history; release
bundles remain draft evidence until machine-release blockers, controller checks,
simulation, setup, quality, and operator or automation signoff clear.

## `GET /fabrication/learning/capabilities`

`GET /learning/capabilities`, `GET /learning/engines/catalog`, and the
gateway-prefixed `GET /fabrication/learning/capabilities` /
`GET /fabrication/learning/engines/catalog` return the live
`dd.fabrication.learning-capability-catalog.v1` catalog for the service's
MDP/POMDP/DES/neural learning surface. The payload identifies the local
`des_engine` crate from `remote/submodules/discrete-event-system.rs`, canonical
MDP/POMDP/DES Studio schema names, `solve_mdp` value-iteration support,
`solve_pomdp_underlying` QMDP-underlying previews, DES Studio
`StudioModelSpec`/`analyze_model_spec` queue graph checks, and
`FeedForwardNetwork` neural-policy sketches. It also lists
`GET /fabrication/learning/outcomes` and `outcomeQualitySurfaces` such as
`learningOutcomes.qualitySummary`, `learningOutcomes.qualityBuckets`,
`learningOutcomes.qualityBuckets.policyUse`, and
`strategyRecommendation.learningOutcomeQuality.releasePolicy` so workers can
discover how compact outcome memory feeds future strategy review. These outputs
remain planning and learning evidence only: machine-ready release stays blocked while validation
findings, unresolved failure boundaries, missing probe evidence, or
human-intervention gates remain open.

## `GET /fabrication/learning/preflight/catalog`

`GET /learning/preflight/catalog` and the gateway-prefixed
`GET /fabrication/learning/preflight/catalog` return the live
`dd.fabrication.learning-preflight-catalog.v1` evidence checklist for promoting
DES, MDP, POMDP, and neural learning outputs into planning previews. The catalog
groups learning outcome artifacts and reward terms, MDP/POMDP belief and policy
state, and neural corpus quality and promotion state. It names response surfaces
such as `learning.outcomes`, `reward_terms`, `mdp-request.desMdpSpec`,
`pomdpBeliefState.hiddenStates`, `releaseProbePlan.probes`,
`neuralTrainingCorpus.examples`, `learningModelResult.promotionBlockers`,
`learningOptimizerResult.candidates`, and `releasePackagePlan.releaseGates`.
Learning preflight does not certify controller output or machine execution:
machine-ready release remains blocked while artifact provenance, reward terms,
belief probes, neural quality evidence, validation, simulation, setup, quality,
operator, or release-package gates are unresolved. Failed checks feed
MDP/POMDP/neural workers so future plans can add evidence, split or combine
work, choose safer machines, regenerate instructions, or require human
intervention earlier.

## `GET /fabrication/learning/features/catalog`

`GET /learning/features/catalog` and the gateway-prefixed
`GET /fabrication/learning/features/catalog` return the live
`dd.fabrication.learning-feature-catalog.v1` feature-map discovery contract for
DES/MDP/POMDP/neural workers. The catalog names `des_engine` from
`remote/submodules/discrete-event-system.rs` as the preferred primitive source
and groups normalized features for plan route/material state,
instruction-validation boundary state, split/combine interface state,
release-evidence and outcome state, and the neural-policy input vector.

Feature groups map signals such as material family, machine class, validation
boundary count, machine-failure boundary kind, automation gap, split/combine
counts, interface evidence, release blockers, reward, `machine-ready`, and
`toolpath-token-sequence` to response surfaces such as `machineSelection`,
`validation.failureBoundaries`, `hybridMakePlan.splitCombineDecisions`,
`releasePackagePlan.releaseGates`, `neuralTrainingCorpus.featureNames`, and
`neuralTrainingCorpus.examples`. Feature vectors are deterministic planning
evidence only: missing CAD/CAM/controller context must remain a blocker or
review-required state, and neural, MDP, POMDP, and DES scores stay advisory
until validation, simulation, setup, quality, operator, and release gates clear.
The payload also exposes `hybridDecisionFeatureContracts` for learning when to
attempt a one-piece build, split work across printed, milled, turned, or
sheet-cut subparts, and recompose interfaces. Those contracts name state
features such as `route-decomposition-action`, `single-piece-feasibility`,
`printed-subpart-count`, `milled-subpart-count`, `turned-subpart-count`,
`interface-criticality`, `datum-transfer-risk`, and `join-process-risk`;
action labels such as `split-for-printing`, `split-for-milling`,
`split-for-turning`, and `combine-after-postprocess`; and release surfaces such
as `interfaceControlPlan.controls`, `assemblyPlan.requiredEvidence`,
`qualityPlan.inspectionPoints`, and `machineRelease.blockers`. Combined parts
remain blocked until child instructions, setup evidence, simulation or dry-run
proof, interface controls, quality evidence, and operator or automation signoff
clear.

## `GET /fabrication/learning/rewards/catalog`

`GET /learning/rewards/catalog` and the gateway-prefixed
`GET /fabrication/learning/rewards/catalog` return the live
`dd.fabrication.learning-reward-catalog.v1` policy-training reward contract for
DES/MDP/POMDP/neural workers. The catalog covers machine-ready release success,
machine-failure boundary penalties, split/combine recovery and route
improvement, human-intervention and automation cost, and evidence-quality
gating.

Each reward family lists reward evidence, reward terms, and training surfaces
such as `learning.outcomes`, `reward_terms`, `mdp_update`, `neural_example`,
`validation.failureBoundaries`, `executionTelemetryResult.machineStops`,
`operatorInterventionPlan.requiredOperatorActions`, `decompositionPlan.parts`,
and `releasePackagePlan.releaseGates`. Reward entries are policy-training
evidence contracts, not controller approval, certified safety validation, or
autonomous release authority. Positive rewards cannot bypass validation,
simulation, quality, setup, telemetry, operator, or release-package blockers.
Reward terms are retained so DES/MDP/POMDP/neural workers can learn which
generated instructions, imported programs, machine choices, split/combine
boundaries, and human checkpoints improved future fabrication outcomes.

## `GET /fabrication/learning/models/catalog`

`GET /learning/models/catalog` and the gateway-prefixed
`GET /fabrication/learning/models/catalog` return the live
`dd.fabrication.learning-model-catalog.v1` retained model-artifact catalog for
DES/MDP/POMDP/neural fabrication learning. The catalog names the local
`des_engine` source crate, canonical MDP/POMDP/DES Studio schemas, and model
families for MDP policy snapshots, POMDP belief policies, DES Studio queue
surrogates, and bounded neural-policy sketches.

Each model family lists artifact kinds, training surfaces, intended planning
uses, and promotion gates. Model artifacts are retained advisory policy evidence,
not controller approvals or autonomous release authority: policy snapshots,
belief-state models, queue surrogates, and neural action scores must remain
replayable from retained job evidence and cannot bypass validation findings,
simulation blockers, setup proof, quality evidence, telemetry review, or
human-intervention gates.

`POST /learning/models/result` and `POST /fabrication/learning/models/result`
accept retained model result reviews from DES/MDP/POMDP/neural workers. The
`dd.fabrication.learning-model-result-review.v1` response normalizes model id,
family, worker, retained artifact URI/checksum, metrics, promotion blockers,
model-card evidence, `modelCardCompatibility`, and learning observations into
the job artifact ledger. Stored jobs retain a dedicated
`learning-model-card-compatibility` artifact that compares neural model-card
feature names and declared input dimensions against the retained
`neuralTrainingCorpus.featureNames` contract.
It also emits a `learning.outcomeDraft` with source job/request ids, reward
hints, model-family hints, promotion status, metric-failure counts, blocker
hints, model-card compatibility status, and artifact hints for
`POST /fabrication/learning/outcomes`. Even accepted model results keep
`machineReady=false`; promotion for future advisory planning requires retained artifacts,
replay verification, metric review, neural model-card compatibility,
and cleared promotion blockers, and remains subordinate to fabrication
validation, simulation, setup, quality, telemetry, and human-intervention gates.

`GET /learning/replay/catalog` and
`GET /fabrication/learning/replay/catalog` return the live
`dd.fabrication.learning-replay-catalog.v1` policy-promotion replay contract for
DES/MDP/POMDP/neural candidates. The catalog names replay sets for
failure-boundary and human-intervention regression, machine route and controller
regression, and reward counterfactuals across retained learning outcomes,
quality buckets, neural examples, simulation findings, toolpath results, release
gates, and split/combine decisions.

Replay evidence must retain source job and candidate ids, artifact URI/checksum
links, baseline and candidate actions, blocker/release-state deltas, DES
MDP/POMDP schema versions, neural feature-map versions, and replay worker
identity. Replay cannot override validation, simulation, quality, setup,
telemetry, release-package, split/combine, or human-intervention gates; accepted
model and optimizer results remain advisory until replay, simulation, artifacts,
and promotion blockers clear through their result review routes.

`GET /learning/beliefs/catalog` and
`GET /fabrication/learning/beliefs/catalog` return the live
`dd.fabrication.learning-belief-catalog.v1` POMDP belief and release-probe
contract. The catalog names the local `des_engine` POMDP primitive, the
`pomdpBeliefState.hiddenStates`, `releaseProbePlan.probes`, and
`mdp-request.desPomdpSpec` surfaces, and the evidence needed to collapse hidden
machine-failure, human-intervention, split/combine, setup, quality, thermal, or
controller uncertainty before release.

Belief probabilities are advisory priors, not machine-ready proof. Unresolved
release probes, validation findings, setup gaps, quality gaps, or
human-intervention gates keep release blocked, and probe outcomes should be
submitted to `POST /fabrication/learning/outcomes` before influencing future
printed, milled, turned, split, or combined jobs.

`GET /learning/optimizers/catalog` and
`GET /fabrication/learning/optimizers/catalog` return the live
`dd.fabrication.learning-optimizer-catalog.v1` optimizer discovery contract for
DES/MDP/POMDP/neural candidate ranking. The catalog names MDP route-action,
POMDP hidden-risk, DES schedule-capacity, and bounded neural-policy optimizer
families, their candidate surfaces, promotion gates, artifact surfaces, and
`POST /fabrication/learning/optimizers/result` handoff. Optimizer candidates are
advisory planning evidence only: promotion requires exactly one selected
candidate, replay and simulation verification, retained URI/checksum artifacts,
satisfied constraints, cleared promotion blockers, and a learning outcome draft.

`POST /learning/optimizers/result` and
`POST /fabrication/learning/optimizers/result` accept retained DES/MDP/POMDP/
neural optimizer candidate reviews. The
`dd.fabrication.learning-optimizer-result-review.v1` response normalizes
candidate actions, scores, expected rewards, risks, selected-candidate proof,
constraint checks, promotion blockers, retained artifacts, and optimizer report
metadata. It stores `learning-optimizer-result`,
`learning-optimizer-candidates`, `learning-optimizer-constraints`,
`learning-optimizer-promotion-blockers`, `learning-optimizer-artifacts`, and
`learning-optimizer-observations`, then emits a
`dd.fabrication.learning-optimizer-learning-outcome-draft.v1` outcome draft for
`POST /fabrication/learning/outcomes`. Optimizer promotion requires exactly one
selected candidate, replay verification, simulation verification, retained
artifacts, satisfied constraints, and cleared promotion blockers; candidate
scores remain advisory and keep `machineReady=false`.

## `GET /fabrication/landing`

`GET /landing` and the gateway-prefixed `GET /fabrication/landing` serve a human
landing page for operators and integration authors. The page explains the
fabrication server's intake-to-release flow: CAD/model/slicer and CAM
intermediate intake, design and machine-code generation, instruction validation
and improvement, split/combine planning, MDP/POMDP/DES/neural learning, retained
job artifacts, and the release gates that keep draft plans from becoming
machine-ready until simulation, controller/postprocessor review, setup, quality,
and operator or automation signoff evidence clear. It also names the native CAD,
cloud CAD, mesh, neutral exchange, and slicer ecosystems accepted by the intake
catalogs, including PTC Creo / Pro/ENGINEER, SOLIDWORKS, Autodesk Fusion,
Siemens NX, CATIA, Onshape, FreeCAD, OpenSCAD, Blender, ZBrush, STEP, Parasolid,
STL, 3MF, OBJ, AMF, PrusaSlicer, OrcaSlicer, Cura, and Bambu Studio, while
calling out ambiguous `.prt`/`.asm` extensions as source-system or translator
evidence gates. The page also includes an operator-facing release-gate matrix
for source provenance, machine envelope, process readiness, simulation evidence,
human or automation handoff, and learning disposition so generated designs,
toolpaths, slicer plans, G-code, controller programs, and text job-sheet
interpretations are visibly advisory until the release packet proves each gate.

## `GET /fabrication/how-it-works`

`GET /how-it-works` and the gateway-prefixed `GET /fabrication/how-it-works`
return the machine-readable companion to the landing page. The
`dd.fabrication.how-it-works.v1` payload gives clients a six-step
intake-to-release flow for discovery, intake, generation, validation, release,
and learning. Each step names the primary routes and the release gate that keeps
draft design packages, generated machine code, printer instructions, imported
CNC/controller streams, text job sheets, split/combine routes, and release
previews from becoming machine-ready without retained evidence. The overview
also lists supported machine families such as 3D printers, vertical mills,
horizontal mills, routers, five-axis systems, mill-turn and Swiss machines,
lathes, sheet cutters, EDM, grinding, inspection, postprocess, assembly cells,
and hybrid split/combine routes. Its `releaseGateMatrix` gives clients the same
source-provenance, machine-envelope, process-readiness, simulation-evidence,
human-or-automation-handoff, and learning-disposition gates described on the
landing page, including the evidence routes and release surfaces each gate can
block.

The `learningContract` section identifies
`remote/submodules/discrete-event-system.rs` / `des_engine` as the preferred
local DES primitive source and keeps MDP, POMDP, DES, neural-policy evidence,
reward replay, and policy promotion advisory until replay, simulation, retained
evidence, and release blockers clear.

## `GET /fabrication/intake/catalog`

`GET /intake/catalog` and `GET /fabrication/intake/catalog` return the
`dd.fabrication.intake-catalog.v1` discovery contract for first-time clients.
The catalog exposes the same `intakeGuide` used by `/fabrication/schema`, with
steps for discovery, CAD/model/slicer review, machine-profile evidence,
instruction analysis or generation, hybrid split/combine planning, and release
plus learning feedback. It also states that intake evidence stays advisory until
retained design, instruction, setup, simulation, quality, release, and learning
reviews clear the machine-ready gates.

## `GET /fabrication/templates/catalog`

`GET /templates/catalog` and `GET /fabrication/templates/catalog` return the
`dd.fabrication.request-templates-catalog.v1` starter-request catalog. The
templates cover FDM printed functional parts, native CAD/3MF intake review for SOLIDWORKS, Creo/ProE, and additive handoff packages, design-to-machine-code generation, direct FDM slicer machine-code generation, direct CNC controller/postprocessor machine-code generation, direct FDM printer instruction generation, direct CNC setup/controller instruction generation, imported CNC dry-run simulation, imported CNC program review, direct imported CNC improvement/patch review, imported slicer G-code review, imported resin/SLA job review, imported powder-bed build review, vertical-mill fixture plates, horizontal-mill side-slot/keyway work, lathe turned inserts, hybrid printed/milled/turned assemblies, direct hybrid decomposition planning, direct hybrid assembly planning, hybrid route costing result feedback, operator intervention result feedback, runtime monitoring result feedback, quality metrology result feedback, release-readiness result feedback, hybrid outcome learning feedback, and boundary-failure learning feedback. Each template
names the target route, including `POST /fabrication/plan`,
`POST /fabrication/design/import/review`,
`POST /fabrication/design/generate`, `POST /fabrication/machine-code/generate`,
`POST /fabrication/instructions/generate`, `POST /fabrication/instructions/analyze`,
`POST /fabrication/instructions/improve`, `POST /fabrication/simulation/run`,
`POST /fabrication/decomposition/plan`, `POST /fabrication/assembly/plan`, and
`POST /fabrication/costing/result`, `POST /fabrication/interventions/result`,
`POST /fabrication/monitoring/result`, and
`POST /fabrication/quality/result`, `POST /fabrication/release/result`, and
`POST /fabrication/learning/outcomes`,
plus preferred manufacturing
methods, machine class, required evidence labels, and a minimal request skeleton
while making clear that templates are not machine-ready instructions. Plan,
design-generation, machine-code-generation, and instruction-generation starter bodies deserialize as
`FabricationPlanRequest` examples, with part `description` and `toleranceMm`
hints where a part list is present. Direct instruction-generation starters keep
FDM slicer/profile, nozzle and bed temperature, extrusion, purge or prime, CNC
controller/postprocessor, tooling, workholding, and dry-run evidence visible
before generated setup sheets or controller handoffs can advance to release
review. The imported CNC dry-run simulation starter also deserializes as a
`FabricationPlanRequest` and keeps imported instructions, machine envelope,
fixture/work-offset review, simulation-risk findings, failure boundaries,
execution stop points, and release-probe learning visible before release. The hybrid printed/milled/turned starter
includes explicit printed-body, milled-datum-pad, and turned-insert part routes
so split/combine and interface-control review starts from concrete child parts.
Direct decomposition and assembly starter bodies reuse those concrete child
routes for `decompositionPlan.routeContracts`,
`decompositionPlan.recompositionInterfaces`, `assemblyPlan.joinOperations`, and
`assemblyPlan.splitCombineDecisions` review.
The hybrid route costing result starter deserializes as a
`CostingResultReviewRequest` example and keeps machine-time/setup estimates,
material yield and scrap allowances, route comparisons, split/combine route
economics, human-intervention cost review, retained artifact checksums, and
`costingLearningOutcomeDraft` feedback visible before future planners promote a
cheaper or safer hybrid route.
The operator intervention result starter deserializes as an
`InterventionResultReviewRequest` example and keeps blocked operator actions,
automation fallback, split/combine interface review, unacknowledged evidence
gates, retained checkpoint artifacts, and `interventionLearningOutcomeDraft`
feedback visible before a machine-ready or unattended release decision.
The runtime monitoring result starter deserializes as a
`MonitoringResultReviewRequest` example and keeps channel heartbeat blockers,
critical alerts, safe-stop/restart recovery actions, operator check-ins,
retained monitoring artifacts, and `monitoringLearningOutcomeDraft` feedback
visible before unattended restart or release decisions.
The quality metrology result starter deserializes as a
`QualityResultReviewRequest` example and keeps out-of-tolerance measurements,
nonconformance findings, blocked inspection gates, retained metrology artifacts,
human disposition or rework/split decisions, and `qualityLearningOutcomeDraft`
feedback visible before machine-ready release.
The release-readiness result starter deserializes as a
`ReleaseReadinessResultReviewRequest` example and keeps blocked release
decisions, retained manifest artifact evidence, machine-ready blockers, pending
human interventions, split/combine release conditions, and
`releaseReadinessLearningOutcomeDraft` feedback visible at the final release
gate.
Design import starter bodies deserialize as
`DesignImportReviewRequest` examples with `designInputs` for native CAD,
ambiguous Creo/NX-style extensions, 3MF packages, translator evidence, units,
topology, PMI, and neutral export review. Imported instruction starter bodies
deserialize as `InstructionAnalysisRequest` examples for Fanuc-style CNC G-code,
Marlin printer G-code, resin job sheets, and powder-bed build packets so the
analyzer path covers G-code and non-G-code fabrication instructions. The direct
instruction-improvement starter uses the same request contract while targeting
`improvedPrograms.patchManifest`, conservative patch review, simulation, and
human approval gates before any repaired controller code can be released. Learning feedback starter bodies deserialize
as `LearningOutcomeRequest` examples with `rewardHint`, manufacturing methods,
and split/combine, machine-failure, and human-intervention observations retained
for MDP/POMDP/neural outcome memory. Template
request skeletons include `templateId` and `templateVersion` trace labels for
job, artifact, release, learning outcome memory, boundary memory, remediation-risk, and neural training evidence. Template entries also include
`releaseGateHints` that point to slicer or controller review, instruction
validation boundaries, design export review gates, direct machine-code generation, slicer-profile and postprocessor handoff evidence, instruction generation, improved-program review, resin postprocess evidence, powder
handling evidence, tooling/workholding evidence, decomposition/interface-control gates, machine-failure and human-intervention blockers, split/combine boundary observations, MDP/POMDP feedback, neural-training examples, release-package gates, and the
`/fabrication/release/catalog` contract.

## `GET /fabrication/schema` And `GET /fabrication/examples`

`GET /schema` and `GET /fabrication/schema` return a compact request contract for
planning, instruction analysis, learning observations, compact learning outcomes,
machine profiles, optional machine-profile evidence, instruction programs, and
response highlights. `GET /examples` and `GET /fabrication/examples` return
ready-to-edit JSON examples for a template-driven FDM request with
`templateId`/`templateVersion` trace labels and release-gate hints, a hybrid
printed/milled/turned plan with calibration/tool/fixture/material/process
evidence, existing CNC and resin-job instruction analysis, outcome learning,
compact learning outcomes, and a NATS instruction-analysis envelope.

The schema also includes an `intakeGuide` for first-time clients: discover the
landing/capabilities/schema/examples routes, review CAD/model/slicer inputs,
attach machine-profile evidence, analyze or generate instructions, plan hybrid
split/combine builds, then release and learn from retained outcome evidence.

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
FDM printer, SLA resin printer, multi-material FDM/toolchanger printer, paste/clay extrusion printer, bound-metal filament FFF printer, material-jetting printer, continuous-fiber composite printer, composite layup cell, SLS powder-bed printer, DED/WAAM directed-energy deposition cell, metal PBF printer, binder jet printer, vertical
mill,
five-axis mill, rotary-indexer mill, horizontal mill, CNC router, laser cutter, waterjet cutter, plasma cutter, wire
EDM cutter, sinker EDM cell, precision grinder, CMM/vision inspection cell, thermal postprocess furnace, surface finishing cell, metal-joining cell, molding/casting cell, composite layup/vacuum-bag/autoclave cell, hot-wire foam cutter, press-brake forming cell, gear-cutting/hobbing/spline-broaching cell, robotic assembly cell, and lathe. If `parts` is omitted, the planner infers
a first decomposition from the objective, material, and tolerance, including
resin-print, multi-material-fdm-print, paste-extrusion-print, bound-metal-fff-print, material-jetting-print, directed-energy-deposition, composite-fiber-print, composite-layup laminate releases, hot-wire foam cores/patterns, binder-jet-print, polymer powder-bed-print, metal PBF-print, five-axis-milled impellers/undercuts,
4th-axis indexed multi-face milling, horizontal-milled side slots/keyways, laser,
waterjet, plasma, wire EDM, sinker EDM cavity burns, precision-grinding datum finishes, CMM/vision first-article inspection releases, thermal anneal/stress-relief/heat-treatment/post-cure releases, assembly-joining/fit-up
steps, surface finishing/coating/plating/anodizing/media-blasting/powder-coating/deburr releases, metal-joining/welding/brazing/soldering releases, molding/casting/vacuum-casting/urethane/silicone releases, composite layup/prepreg/wet-layup/vacuum-bag/autoclave/resin-infusion releases, press-brake sheet-metal flanges/bend sequences/formed brackets, gear teeth/splines/racks/keyways/worm profiles, and kerf-controlled
sheet-cut profiles, and routed sheet/profile parts for wood,
foam, acrylic, panel, sign, engraving, and tabbed-profile requests. Additive
plans flag overhang, bridge, cantilever, thin-wall, snap-fit, and resin
drain/cupping geometry as review boundaries before draft machine instructions
are treated as releasable.

## `GET /fabrication/workflow/catalog`

`GET /workflow/catalog` and the gateway-prefixed
`GET /fabrication/workflow/catalog` return the live
`dd.fabrication.workflow-catalog.v1` orchestration catalog before a caller asks
for a workflow plan. The payload lists stage IDs for design intake/conversion,
machine/material routing, split/combine assembly, instruction and machine-code
generation, validation/remediation/simulation, setup/quality/monitoring/
postprocess readiness, and execution/release/learning feedback. Each stage
declares route handoffs, result handoffs, response surfaces, evidence surfaces,
and release gates.

The catalog also exposes `workerCatalogRoutes`, `resultReviewCatalogRoutes`,
`learningCatalogRoutes`, `learningOutcomeRoutes`, and `stageResultHandoffs` so
orchestration clients can connect design conversion, instruction/machine-code
generation, validation, remediation, simulation, execution, release, and
learning stages to the worker lanes, result-review intake routes,
DES/MDP/POMDP/neural learning catalogs, and retained learning outcome
memory/submission routes that consume their artifacts and outcomes.

Workflow catalog entries are route and evidence contracts, not certified machine
release. Machine-ready release remains blocked until the matching workflow plan,
retained artifacts, validation or simulation proof, controller/setup/quality
evidence, and operator or automation signoff clear. The catalog names
`workflow-plan`, `instruction-intent-map`, and `mdp-request` artifact surfaces so
MDP/POMDP/DES/neural workers can learn where future jobs should reroute,
split/combine, regenerate, remediate, or insert human checkpoints.

## `POST /fabrication/workflow/plan`

`POST /workflow/plan` and the gateway-prefixed
`POST /fabrication/workflow/plan` accept the same request body as
`POST /fabrication/plan`, apply bounded learning-policy memory, store and publish
the normal plan outputs, and return a compact
`dd.fabrication.workflow-planning.v1` route-and-evidence manifest. The response
wraps `workflowPlan.stages` for design intake/conversion, machine/material
routing, split/combine assembly, instruction and machine-code generation,
validation/remediation/simulation, setup/quality/monitoring/postprocess, and
execution/release/learning handoffs. Each stage carries `routeHandoffs`,
`resultHandoffs`, response surfaces, evidence surfaces, blocked status, and the
current learning-policy snapshot.

Workflow plans are orchestration evidence, not machine-ready release. They keep
`machineReady=false` while stage blockers, generated-program drafts,
machine-release blockers, missing validation/simulation evidence, release-package
gates, or operator and automation signoffs remain open, and they expose
`workflow-plan`, `instruction-intent-map`, and `mdp-request` artifact surfaces so
MDP/POMDP/DES/neural workers can learn where to reroute, split/combine,
regenerate, remediate, or add human checkpoints.

The plan-level `instructionIntentMap` is retained as the `instruction-intent-map`
artifact and embedded in the optimizer-shaped `mdp-request`. Workflow stages use
it as an instruction/machine-code evidence surface so generated programs and
submitted existing instructions share the same intent, review-priority,
machine-failure, human-intervention, split/combine, and learning-feedback queues.

The response also includes `workflowActionQueue` and
`workflowPlan.actionQueue`, derived from blocked workflow stages. Queue entries
turn stage blockers into explicit next actions such as
`generate-or-review-machine-instructions`,
`analyze-remediate-and-simulate-before-release`,
`resolve-split-combine-interface-control`, and
`hold-release-and-record-learning-outcome`. Each action carries route handoffs,
result handoffs, response and evidence surfaces, a `learningSignal` such as
`workflow-action:validation-remediation-simulation`, and a release policy that
requires the action or an explicit waiver/review record before machine-ready
release.

## `POST /instructions/analyze`

`POST /fabrication/instructions/analyze` and its gateway-stripped alias
`POST /instructions/analyze` accept existing G-code-like programs plus
non-controller text instructions such as printer job sheets, setup sheets, and
operator checklists. It returns controller-agnostic safety findings, improvement
opportunities, and `improvedPrograms` review drafts that insert conservative
modal defaults or explicit setup, post-processing, split, assembly, and
human-intervention gates. Each improved program includes a `patchManifest` with
draft insert/review operations such as `insert-before-line`,
`insert-before-program`, `insert-after-program`, and `review-line`, plus content
snippets, `apply-instruction-patch-*` policy actions, and `instruction-patch:*` learning observations for MDP/POMDP/neural workers. Submitted machine profiles are bounded and validated,
including positive work-envelope values, unique IDs, nonzero axis counts, and
bounded non-secret `profileEvidence` lists for calibration, tools, fixtures,
materials, process support, maintenance, release evidence, and retained blockers.

Plan responses include `instructionIntentMap` for generated plus submitted
instruction streams and retain it as `instruction-intent-map`; analysis responses
include the same contract for submitted streams and retain it as the
`analysis-instruction-intent-map` artifact. The map normalizes each submitted
program into primary process intents such as additive print, subtractive
machining, turning, joining, inspection, or general instruction work; language
counts; machine-failure watchpoints; human-intervention watchpoints;
split/combine hints; release handoff routes; and `instruction-intent:*`
learning observations. Its `reviewPriorities` array names the highest-value
review queues first: machine-failure boundaries, human-intervention gates,
split/combine or interface reviews, non-G-code job-sheet evidence extraction,
and learning feedback after disposition. Each row lists triggering evidence,
review order, response surfaces such as
`instructionIntentMap.programs.machineFailureWatchpoints`,
`operatorInterventionPlan.requiredOperatorActions`, `decompositionPlan`, and
`learning.outcomeDraft`, plus the release policy that keeps `machineReady=false`
until retained validation, simulation or dry-run, provenance, and operator or
automation signoff clear. These intent maps are routing and learning evidence
only: validation, simulation or dry-run, provenance, and operator or automation
signoff still gate any machine-ready release.

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

## `POST /instructions/validate`

`POST /fabrication/instructions/validate` and its gateway-stripped alias
`POST /instructions/validate` accept the same request body as
`POST /fabrication/instructions/analyze`, run the same validation, boundary,
simulation, release, and improvement pipeline, persist the same analysis
artifacts, and return the compact
`dd.fabrication.instruction-validation.v1` validation envelope. The payload
focuses on `validation.findings`, `validation.failureBoundaries`,
`boundarySummary`, `resolutionPlan.steps`, `interventionMap`,
`operatorInterventionPlan`, `machineRelease.blockers`,
`executionPlan.programRuns`, `postprocessPlan.controllerTargets`,
`simulation.failureBoundaries`, `improvements`, `improvedPrograms.patchManifest`,
and DES/neural learning surfaces. It is a validation and release-blocking
contract, not controller certification: submitted CNC, printer, slicer,
job-sheet, setup, and operator instructions keep `machineReady=false` while
validation findings, simulation findings, machine-failure boundaries,
human-intervention gates, split/combine reviews, or release blockers remain
open.

## `POST /instructions/validation/result`

`POST /fabrication/instructions/validation/result` and its gateway-stripped alias
`POST /instructions/validation/result` accept retained external validation
results for CNC, printer, slicer, setup-sheet, postprocess, assembly, and
operator instruction streams. Workers submit validator identity, instruction
identity, language, optional machine/controller context, findings, failure
boundaries, improvement suggestions, retained artifacts, warnings, and metadata.

The response normalizes those results into
`dd.fabrication.instruction-validation-result-review.v1`, reports
`validationResultJobId`, `instructionValidationResult`, `releaseBlocked`,
`machineReady`, `releaseReady`, finding/boundary/improvement blocker counts,
human-intervention and split/combine flags, artifact evidence gaps, follow-up
validation/simulation/release routes, learning observations, and a
`priorityDispositions` array that mirrors `instructionIntentMap.reviewPriorities`
for machine-failure, human-intervention, split/combine or interface,
non-G-code job-sheet evidence, and learning-feedback lanes. The
`dd.fabrication.instruction-validation-learning-outcome-draft.v1` payload carries
the same priority dispositions with language, controller, finding, boundary,
improvement, blocker, split/combine, and recommended-action hints for
`POST /fabrication/learning/outcomes`.
Review jobs retain `instruction-validation-result`,
`instruction-validation-findings`, `instruction-validation-boundaries`,
`instruction-validation-improvements`, `instruction-validation-artifacts`,
`instruction-validation-priority-dispositions`, and
`instruction-validation-learning-observations` artifacts. Release remains
blocked until validation blockers, human/split/combine boundaries, retained
URI/checksum/evidence artifacts, simulation or dry-run evidence, controller
review, and signoff clear. Validation observations include
`instruction-validation-priority:<priority>:<disposition>` so MDP/POMDP/neural
workers can learn which imported instruction validators closed or left open each
review-priority lane.

## `POST /instructions/improve`

`POST /fabrication/instructions/improve` and its gateway-stripped alias
`POST /instructions/improve` accept the same request body as
`POST /fabrication/instructions/analyze`, run the same validation and simulation
pipeline, persist the same analysis artifacts, and return the compact
`dd.fabrication.instruction-improvement-review.v1` patch-review envelope. The
payload focuses on changed program counts, patch operation counts,
`improvedPrograms.patchManifest`, release blockers, `machineReady=false` review
state, and `instruction-patch:*`/`instruction-patch-action:*` learning surfaces
for downstream MDP/POMDP/neural workers. It also includes a
`dd.fabrication.instruction-improvement-learning-outcome-draft.v1` payload with
changed-program, patch-operation, human-review, improvement-action, boundary,
patch-action, reward, and submit-route hints for
`POST /fabrication/learning/outcomes`.

Each `improvedPrograms.patchManifest` includes a `reviewSummary` that classifies
draft patch operations as modal/controller-state, process-evidence,
split-combine/interface, human-review, or general-review work. The summary keeps
`linePatchCount`, `humanReviewCount`, `machineCodePatch`, `releasePosture`, and
route hints for validation, simulation, boundary review, intervention, assembly,
decomposition, and release-result lanes so generated patches remain review
evidence rather than machine-ready edits.

## `POST /instructions/boundaries/review`

`POST /fabrication/instructions/boundaries/review` and its gateway-stripped
alias `POST /instructions/boundaries/review` accept the same instruction-analysis
request body and return the compact
`dd.fabrication.instruction-boundary-review.v1` envelope. It focuses on
`boundarySummary`, `resolutionPlan`, `interventionMap`,
`operatorInterventionPlan`, `machineRelease`, `executionPlan`, and
`simulation.riskProfile` so callers can review where submitted CNC, printer,
slicer, setup-sheet, or operator instructions will fail without human
intervention, verified automation, regeneration, split/combine work, or release
evidence. The full analysis artifacts are still stored and published, and
boundary observations such as `boundary-kind:*`, `resolution-action:*`, and
`interventionMap.learningObservations` remain available to MDP/POMDP/neural
workers. It also includes a
`dd.fabrication.instruction-boundary-learning-outcome-draft.v1` payload with
boundary, resolution-action, human-intervention, split/combine, automation
fallback, risk-score, reward, and submit-route hints for
`POST /fabrication/learning/outcomes`.

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
controls, slicer profile/support/orientation/first-layer evidence, missing slicer mesh unit/scale/watertight/manifold/normals/wall-thickness evidence for STL/3MF/OBJ/model inputs, slicer high-speed input-shaper/acceleration/volumetric-flow evidence, post-processing, missing multi-material FDM material/color map/slot/filament-lot/support-interface evidence, missing multi-material FDM purge/wipe tower/tool-change/runout-sensor/resume-state evidence, missing pellet/FGF pellet-lot/drying/moisture/hopper/purge/nozzle evidence, missing pellet/FGF bead width/layer height/screw/melt/cooling/gantry-clearance/warpage/trim evidence, missing robotic additive robot-frame/TCP/reach/collision/interlock/external-axis/dry-run evidence, missing robotic additive feedstock/nozzle/purge/bead/flow/cooling/cure/dimensional-scan evidence, missing paste/clay rheology/slump/deairing/nozzle/pressure evidence, missing paste/clay drying/humidity/shrinkage/green-part/firing evidence, missing bound-metal filament lot/profile/hardened-nozzle/dry-storage/shrinkage-scale evidence, missing bound-metal debind/brown-part/sinter-furnace/atmosphere/shrinkage-coupon/density evidence, missing material-jetting cartridge/material-channel/printhead/tray evidence, missing material-jetting support-removal/UV/color/material inspection evidence, missing DED/WAAM feedstock/substrate/bead-path/standoff/machining-allowance evidence, missing DED/WAAM energy/shielding/melt-pool/interpass/NDE/coupon evidence, missing composite-fiber layup/orientation/load-case evidence, missing composite-fiber spool/cutter/matrix/coupon/continuity inspection evidence, missing composite layup mold/mandrel/release-film/ply-schedule/resin-prepreg-core-lot/out-time evidence, missing composite layup vacuum-bag/leak-down/debulk/cure-trace/demold/trim-drill/coupon/NDI/dimensional-release evidence, missing hot-wire foam setup evidence for foam density/blank thickness/template-or-CNC-profile/bow-wire tension/fume extraction/PPE/fire watch, missing hot-wire foam process evidence for wire heat/current/feed/kerf/wire-lag/taper/surface-melt/dimensional inspection, missing resin exposure/profile/layer/support/build-plate evidence, missing resin layer/exposure manifest image hash/checksum or peel/lift/recoat evidence, missing resin vat-volume/level/refill evidence for large resin jobs, resin IPA/wash/cure/drain/PPE/
waste controls or missing resin postprocess evidence, powder
build profile/powder lot/nesting controls or missing powder-bed build/profile evidence,
cooldown/depowder/recovery controls or missing powder-bed handling evidence, missing
metal-PBF alloy-lot/oxygen/recoater/stress-relief/plate-removal evidence, missing
powder-bed recoater clearance/thermal spacing/cooldown evidence, missing binder-jet binder/saturation/printhead/green-strength evidence, missing binder-jet cure/debind/sinter/infiltration/shrink-compensation evidence, assembly
dry-fit/metrology/datum/torque/cure controls or missing assembly fit/metrology evidence, missing assembly-cell robot-path/gripper/fixture/vision/interlock evidence, missing assembly-cell press/heat-set/torque/adhesive/cure/final-metrology evidence, missing part-separation cut-path/fixture/kerf/heat/deburr/traceability/final-inspection evidence, missing part-separation retained-tab release/deburr/traceability/final-inspection evidence, missing precision tolerance/surface-finish metrology evidence, missing unattended/batch monitoring and recovery evidence, missing unattended/batch restart/recovery/operator-check-in evidence, missing thermal postprocess temperature/furnace/atmosphere/cooldown/quench/inspection evidence, missing surface/chemical finishing media/masking/PPE/waste/thickness/inspection evidence, missing metal-joining WPS/procedure/qualification/filler/flux/gas/fit-up/fume-control/heat-input/interpass/NDE/repair-disposition evidence, missing molding/casting master/tool/mold-material/parting/vent-gate/release-agent/mix-ratio/pot-life/degas/vacuum/pressure/cure/demold/shrinkage/void/dimensional-release evidence, missing plastic-joining polymer-compatibility/joint-design/energy-director/staking-boss/fixture-nest/weld-stake-solvent-recipe/collapse/melt-flow/cooling evidence, missing plastic-joining weld-collapse/stake-head/flash/cracks-crazing/proof/leak-or-visual/dimensional-fit/first-article release evidence, missing dynamic-balancing rotor-identity/balance-grade/target-speed/fixture-arbor/sensor-calibration/trial-weight evidence, missing dynamic-balancing residual-unbalance/vibration-spectrum/correction-retention/first-article/release evidence, missing composite layup mold/mandrel/release-film/ply-schedule/resin-prepreg-core-lot/out-time/vacuum-bag/leak-down/debulk/cure-trace/demold/trim-drill/coupon/NDI/dimensional-release evidence, missing press-brake/sheet-forming flat-pattern/bend-allowance/tooling/tonnage/backgauge/springback/angle-inspection evidence, missing gear-cutting/hobbing/spline-broaching gear-drawing/tooth-count/module-or-DP/pressure-angle/helix-lead/cutter-arbor/index-ratio/blank-runout/deburr/over-pins/span/profile/backlash inspection evidence, missing indexed setup clamp/brake/index-angle/clearance/re-probe evidence, unreviewed `G51` scaling/mirroring or `G68` coordinate rotation and missing `G50.1`/`G69` transform cancellation, `G43.4`/`G234` tool-center-point mode before rotary/linear motion or program end without TCP kinematic review and `G49` cancellation, `G92` work-coordinate offsets before motion or program end without temporary-offset review and `G92.1`/`G92.2` cancellation, `G10 L2`/`G10 L20` fixture/work-offset table writes without controller offset-table backup or review evidence, late or mid-program `G20`/`G21` unit-mode changes after motion without conversion review, sheet-cutting
kerf/fire/fume checks or missing sheet-cutting material/thickness/cut-chart recipe evidence, missing generated sheet-cutting setup/cut-path/release evidence, missing wire EDM start-hole/threading/slug-retention/dielectric/flushing/skim-pass evidence, wire EDM profile/skim cuts before start-hole, wire-threading, guide/tension, conductive workholding, or slug-retention setup evidence, missing sinker EDM electrode/dielectric/flushing/debris-removal/depth/orbit-finish/recast evidence, missing precision grinding wheel dress/balance/coolant/workholding/spark-out/burn-check/surface-finish/final-metrology evidence, missing CMM/vision inspection probe or vision calibration, datum alignment, uncertainty, measured-values, pass/fail disposition, nonconformance-routing evidence, missing mill-turn live-tooling C-axis/Y-axis/polar-interpolation evidence, missing mill-turn subspindle pickup/clamp/sync/pull-force/transfer-clearance evidence, missing Swiss guide-bushing/bar-feed/collet/remnant evidence, missing Swiss gang-tool/live-tool clearance, missing Swiss subspindle pickoff/cutoff/ejection/runout evidence, `G4`/`G04` dwell commands without positive `P`/`S`/`X`/`U` duration or operator-timed dwell review, lathe text threading feed-per-rev/pitch/spindle-encoder evidence, lathe text part-off catcher/subspindle/tailstock/stock-support evidence, assembly, splitting, or operator intervention. Improved
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
risks are reviewed before release. Each simulation report also carries a
`riskProfile` with per-program risk scores, high-risk counts, recommended
actions, and learning observations such as `simulation-risk:*` so MDP/POMDP and
neural workers can learn which motion traces need machine-failure or
human-intervention gates.

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
split/combine `interfacePlan` objects with joint type, fit, decomposition
strategy, and inspection gate details, automation paths, program boundary traces,
and machine-failure risk scores back to program IDs and process nodes
when process graph context exists.
Instruction-analysis responses also include a `learning` plan derived from the
submitted programs, boundary evidence, improvements, and release probes. The
retained `analysis-learning-plan`, `analysis-pomdp-belief-state`,
`analysis-release-probe-plan`, `analysis-neural-training-corpus`,
`analysis-des-instruction-model`, and `analysis-mdp-request` artifacts let
MDP/POMDP, DES, and neural workers learn from imported CNC, slicer, printer, and
text instruction streams without requiring a new generated design plan first.

Plan responses also include `assembly.assemblyGraph`; the retained
`parametric-design` and `assembly-plan` artifacts carry the same graph so
external CAD/CAM or learning workers can connect generated parts, manufacturing
methods, join interfaces, dry-fit/metrology gates, and assembly sequence steps.
The response, retained `hybrid-make-plan`, retained `parametric-design`, and
`mdp-request` artifacts include `hybridMakePlan` routes, join operations,
split/combine decision records, review gates, and learning observations so
policy workers can learn whether to keep geometry as one piece, split it for
machining/printing, or recombine it through inspected assembly interfaces.
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
The response, retained `material-plan`, retained `parametric-design`, and
`mdp-request` artifacts include `materialPlan` route feedstock, stock forms,
quantity estimates, scrap allowances, conditioning steps, required evidence,
release gates, blockers, and learning observations so material/stock/feedstock
planning state is visible to CAM, slicer, operator, and policy workers.
The response, retained `production-plan`, retained `parametric-design`, and
`mdp-request` artifacts include `productionPlan` quantity-aware batch data so
schedulers can compare batch counts, setup repeats, estimated machine minutes,
review gates, blockers, and unattended-run eligibility.
The response, retained `machine-schedule`, retained `parametric-design`, and
`mdp-request` artifacts include `machineSchedule` machine-lane utilization,
operation windows, dependency holds, postprocessor holds, and operator or
automation start gates so resource sequencers can see where generated work
cannot enter a printer, mill, mill-turn or Swiss center, router, sheet cutter, lathe, or
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
The response, retained `fixture-plan`, retained `parametric-design`, and
`mdp-request` artifacts include `fixturePlan` setup strategies, datum schemes,
workholding, required evidence, clearance checks, datum-transfer records,
automation candidates, release blockers, and `fixture-*` learning observations
so CAM, fixture, operator, simulation, and policy workers can decide whether a
route can run unattended or needs setup intervention before machine release.
The response, retained `monitoring-plan`, retained `parametric-design`, and
`mdp-request` artifacts include `monitoringPlan` runtime channels, expected
signals, alert rules, recovery actions, release blockers, and `monitoring-*`
learning observations so live machine evidence can be tied back to generated
programs, safe-stop gates, and outcome learning.
The response, retained `interface-control-plan`, retained `parametric-design`,
and `mdp-request` artifacts include `interfaceControlPlan` join/split interface
controls, mating-surface evidence, acceptance criteria, split/combine decision
links, release gates, blockers, and `interface-*` learning observations so
hybrid planners can verify where parts may be combined, separated, dry-fit, or
reworked before machine-ready release.
The response, retained `decomposition-plan`, retained `parametric-design`, and
`mdp-request` artifacts include `decompositionPlan` split targets, route
contracts, recomposition interfaces, release gates, release blockers, and
`decomposition-*` learning observations so CAD/CAM, slicer, robot, operator, and
policy workers can decide whether a job completes as one piece, separated child
routes, or a recomposed assembly.
The response plus retained `machine-selection`, `parametric-design`, and
`mdp-request` artifacts include `machineSelection` candidate scores and selected
machine reasons so learning workers can compare alternate printers, mills,
routers, mill-turn or Swiss centers, wire EDM, sinker EDM, other sheet/special-process
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
The response, retained `controller-plan`, retained `parametric-design`, and
`mdp-request` artifacts include `controllerPlan` compatibility targets, dialect
summaries, postprocessor-known status, required controller checks, required
evidence, controller release gates, release blockers, and `controller-*`
learning observations so printer, mill, router, sheet cutter, lathe, mill-turn, Swiss,
EDM, and assembly-cell programs stay blocked until exact-controller review,
dry-run evidence, and signoff are attached.

## Outcome Learning

`POST /fabrication/learning/observe` accepts completed or failed fabrication
outcomes for a generated plan, program, part, machine, or external shop-floor
instruction stream. `POST /learning/observe` is the gateway-stripped alias.
When `sourceJobId` points at a retained fabrication-plan job, sparse outcomes
are enriched from the stored plan before reward shaping: missing `programId`,
`partId`, `machineId`, `machineKind`, `material`, and `operationSequence`
fields can be filled from the generated program, design part, process plan, and
hybrid make plan. The resulting observations include `source-plan-*` signals so
MDP/POMDP/neural workers can trace learned method combinations back to the plan
evidence that produced them.

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
  sequences, machine-kind preferences, assembly preferences, first-class
  `splitCombinePreferences`, and
  material-specific `remediationRisks` from failed or negative fabrication
  evidence.

Learning outcomes are also recorded as job artifacts: `outcome-learning-event`,
`reward-signal`, `mdp-experience`, `outcome-remediation-plan`,
`pomdp-observations`, and `neural-example`.
`GET /fabrication/learning/policy` and `GET /learning/policy` return the
`dd.fabrication.learning-policy-snapshot.v1` self-describing policy snapshot:
route aliases, local `des_engine`/`remote/submodules/discrete-event-system.rs`
provenance, MDP/POMDP/DES Studio schema names, neural primitive identity,
summary counts, detailed learned preference surfaces, and advisory
`promotionPolicy` notes. `GET /fabrication/learning/corpus` and
`GET /learning/corpus` return the `dd.fabrication.learning-corpus.v1`
self-describing training corpus: route aliases, MDP/POMDP schema names,
`FeedForwardNetwork` provenance, neural training examples, boundary learning
examples, remediation risks, and release-policy notes that validation,
simulation, controller, setup, quality, and signoff gates remain authoritative.
`GET /learning/outcomes` and
`GET /fabrication/learning/outcomes` return the
`dd.fabrication.learning-outcome-memory.v1` bounded outcome memory, including
retained compact/rich learning records, `maxOutcomes`, a `qualitySummary`,
`qualityBuckets` for successful-positive-reward, failed-or-negative-reward,
hybrid split/combine, and intervention-heavy history, the derived policy
snapshot, `policyImpactPreview` entries for method-combination,
machine-kind, operation-sequence, split-combine-preference, remediation-risk, and neural-training impacts,
and release-policy notes that learned preferences remain advisory until
validation, simulation, controller review, and signoff evidence clear.
`POST /learning/outcomes` and `POST /fabrication/learning/outcomes` accept a
compact success/reward record when callers already have their own training
features. They also accept the `learning.outcomeDraft` payloads emitted by
result endpoints directly: `sourceRequestId`, `sourceJobId`, and `rewardHint`
map to the compact record identity/reward fields, while `sourceKind`, `*Hints`,
and `featureHints` are retained as normalized learning observations and
`manufacturingMethodHints` can seed learned method preferences. Assembly,
decomposition, and strategy result drafts can also use `joinKindHints` and
`splitCombineHints` to seed learned assembly strategies and first-class
`splitCombinePreferences` for future hybrid split/combine plans.

When a policy snapshot has at least two positive samples for a method such as
`additive-print`, `milling`, `five-axis-milling`, `horizontal-milling`, `routing`,
`sheet-cutting`, `plastic-joining`, or `turning`, subsequent
`/fabrication/plan` requests without explicit `preferredMethods` inherit those
learned process preferences. Repeated multi-method successes such as
`additive-print+milling` or `additive-print+plastic-joining` are retained as
method combination preferences; open future requests can be decomposed into
learned hybrid parts before machine selection. Learned join-cell combinations
keep each generated part's explicit preferred method authoritative so a
plastic-joining policy can add a join lane without stealing the printed part's
printer assignment. Repeated ordered successes such as
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
Outcome observations that identify validation or execution boundaries, such as
`boundary-kind:*`, `boundary-severity:*`, `resolution-action:*`,
machine-failure, or human-intervention-required signals, are also retained in
policy snapshots as `boundaryLearningExamples`. Matching future plans replay
those examples as `boundary-memory` training inputs and normalized
`learned-boundary-memory:*` observations so the neural corpus and POMDP state can
learn which machine-envelope, split/combine, or intervention boundaries blocked
prior jobs.
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
`neuralPolicy` sketch with a normalized feature vector, hidden activations,
`neuralPolicy.engineInference` from the local DES `FeedForwardNetwork`,
parameter counts, output scores, top signal, and bounded action scores lets an
external neural model train from the same state or replace the local scoring
head. `neuralTrainingCorpus` carries per-part generated examples,
per-boundary `validation-boundary` examples linked to resolution actions,
`instruction-patch` examples for line-level repair actions, policy-memory examples
including replayed `boundary-memory`, bounded labels, and strategy inference
candidates aligned to `neuralFeatures`. `interventionSignals` expose automation requirements and ordered
`resolutionPlan` steps as learnable actions, observations, next states, and
reward adjustments. The optimizer-shaped `mdp-request` artifact includes
`learningEngine`, `desMdpSpec`, `desMdpSolution`, `desPomdpSpec`,
`desPomdpSolution`, `strategyCandidates`, `interventionSignals`, `pomdpBeliefState`,
`releaseProbePlan`, `neuralTrainingCorpus`,
`designPackage`, `designExports`, `designInputReview`, `productionPlan`,
`machineSchedule`, `desScheduleModel`, `machineSelection`, `manufacturingHandoff`,
`materialPlan`, `qualityPlan`, `toolingPlan`, `fixturePlan`, `monitoringPlan`, `interfaceControlPlan`, `decompositionPlan`, `processGraph`, `hybridMakePlan`, `interventionMap`, `executionPlan`, `postprocessPlan`, `controllerPlan`, `simulation`,
`automationRequirements`, `resolutionPlan`, and `machineRelease` so external
MDP/POMDP workers can reuse the same DES-backed policy preview, boundary evidence, design export state,
CAD/model/slicer source assumptions, batch-planning state, machine-choice
alternatives, machine-schedule state, DES queue-capacity model, material/stock/feedstock planning state, hybrid make/split decisions, simulation risk profiles, quality evidence targets, tooling/setup, fixture/setup, runtime monitoring
requirements, intervention paths, postprocessor gates, and CAD/CAM handoff
assumptions.
Core handoff fields include `designPackage`, `designExports`, `designInputReview`, `productionPlan`,
`machineSchedule`, `desScheduleModel`, `machineSelection`, `manufacturingHandoff`,
`materialPlan`, `qualityPlan`, `toolingPlan`, `fixturePlan`,
`monitoringPlan`, `interfaceControlPlan`, and `releasePackagePlan`.

## Job And Artifact Inspection

Every successful planning, instruction-analysis, learning-observation, or
learning-outcome request is recorded in a
bounded in-process ledger. This is not durable storage yet; it is the current
runtime inspection boundary while the database contract is still being designed.

- `GET /jobs/catalog` returns the
  `dd.fabrication.job-evidence-catalog.v1` route, retention, artifact-family,
  release-bundle, and learning-surface contract for the bounded ledger.
  `GET /fabrication/jobs/catalog` is the gateway-prefixed app alias.
- `GET /jobs` lists retained jobs with status, severity, summary, and artifact
  IDs. `GET /fabrication/jobs` is the gateway-prefixed app alias for the same
  ledger.
- `GET /jobs/:job_id` returns the recorded plan or analysis response plus
  artifact summaries. `GET /fabrication/jobs/:job_id` is the prefixed alias.
- `GET /jobs/:job_id/release-bundle` returns the
  `dd.fabrication.job-release-bundle.v1` packet for a retained job, including
  full design-package/export, generated machine-code, improved-program,
  release-package, machine-release, simulation, setup, quality, controller,
  postprocess, POMDP belief, release-probe, neural-corpus, and `mdp-request`
  artifacts. The bundle also includes `bundleManifest`, a compact category
  checklist for design/source, machine-code/instruction, setup/execution,
  simulation/quality/release, and learning/policy feedback evidence, with
  present/missing counts in `summary`. `GET /fabrication/jobs/:job_id/release-bundle`
  is the prefixed alias. Bundles are draft operator/worker evidence packets and
  remain blocked until release gates clear.
- `GET /jobs/:job_id/artifacts/:artifact_id` returns one full artifact payload,
  with `GET /fabrication/jobs/:job_id/artifacts/:artifact_id` as the prefixed
  alias,
  such as `design-summary`, `parametric-design`, `process-plan`,
  `design-package`, `design-export-bundle`, `design-input-review`, `production-plan`, `machine-schedule`, `des-schedule-model`, `machine-selection`, `process-graph`, `hybrid-make-plan`,
  `manufacturing-handoff`, `material-plan`, `quality-plan`, `tooling-plan`, `fixture-plan`, `monitoring-plan`, `interface-control-plan`, `decomposition-plan`, `release-package-plan`, `machine-release`,
  `execution-plan`, `postprocess-plan`, `controller-plan`, `boundary-summary`, `intervention-map`, `simulation-report`, `learning-plan`,
  `pomdp-belief-state`, `release-probe-plan`, `neural-training-corpus`,
  `mdp-request`, a `generated-design-export`, a `program-*` generated machine program, or an
  `improved-program-*` instruction rewrite, plus instruction-analysis artifacts such as
  `analysis-boundary-summary`, `analysis-intervention-map`,
  `analysis-machine-release`, `analysis-execution-plan`, `analysis-postprocess-plan`,
  `analysis-simulation-report`, `analysis-des-instruction-model`, `analysis-learning-plan`,
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
  `assemblyGraph` nodes, interfaces, and sequence gates; `hybrid-make-plan`,
  `parametric-design`, and `mdp-request` include `hybridMakePlan` part routes,
  join operations, split/combine decisions, review actions, and learning
  observations; `parametric-design`,
  `production-plan`, and `mdp-request` include `productionPlan` batch counts,
  setup repeats, estimated machine minutes, review gates, release blockers, and
  unattended-run eligibility; `des-schedule-model`, `machine-schedule`,
  `parametric-design`, and `mdp-request` include `machineSchedule`,
  `desScheduleModel` DES Studio queue blocks, service-rate estimates, structural
  analysis, lane mappings, operation windows, dependency holds, postprocessor
  holds, and operator/automation start gates;
  `parametric-design`,
  `machine-selection`, and `mdp-request` include `machineSelection` candidate
  scoring, selected-machine reasons, and rejection/review status for each part;
  `parametric-design`,
  `manufacturing-handoff`, and `mdp-request` include `manufacturingHandoff`
  part-level stock, datum, fixture, program-link, inspection, and release-blocker
  data; `material-plan`, `parametric-design`, and `mdp-request` include
  `materialPlan` route feedstock, stock forms, quantity estimates, conditioning
  steps, required evidence, release gates, blockers, and learning observations;
  `quality-plan` and `mdp-request` include `qualityPlan` inspection
  points, measurement targets, records to capture, release gates, and learning
  observations; `tooling-plan` and `mdp-request` include `toolingPlan` required
  tools, workholding, consumables, setup checks, automation dependencies,
  production-batch links, and release blockers; `fixture-plan`,
  `parametric-design`, and `mdp-request` include `fixturePlan` setup strategies,
  datum schemes, workholding, required evidence, clearance checks, datum-transfer
  records, automation candidates, release blockers, and `fixture-*` learning
  observations; `monitoring-plan`, `parametric-design`, and `mdp-request` include
  `monitoringPlan` runtime channels, expected signals, alert rules, recovery
  actions, release blockers, and `monitoring-*` learning observations;
  `interface-control-plan`, `parametric-design`, and `mdp-request` include
  `interfaceControlPlan` join/split interface controls, mating-surface evidence,
  acceptance criteria, split/combine decision links, release gates, blockers,
  and `interface-*` learning observations;
  `decomposition-plan`, `parametric-design`, and `mdp-request` include
  `decompositionPlan` split targets, route contracts, recomposition interfaces,
  release gates, blockers, and `decomposition-*` learning observations;
  `release-package-plan`, `parametric-design`, and `mdp-request` include
  `releasePackagePlan` machine-program and assembly/recomposition packages,
  design export IDs, controller target IDs, fixture setup IDs, monitoring point
  IDs, quality inspection IDs, decomposition target IDs, interface-control IDs,
  required artifacts, release gates, blockers, and `release-package*` learning
  observations;
  `intervention-map`,
  `analysis-intervention-map`, and `mdp-request` include `interventionMap`
  human-intervention points, split/combine decisions with `interfacePlan`
  decomposition/recombination gates, automation paths, program boundary traces,
  learning observations, and machine-failure risk
  scores; `execution-plan`, `analysis-execution-plan`,
  `parametric-design`, and `mdp-request` include `executionPlan` program runs,
  checkpoints, execution stop points, unattended-run eligibility, and required
  intervention or automation actions;
  `postprocess-plan`, `analysis-postprocess-plan`, `parametric-design`, and
  `mdp-request` include `postprocessPlan` controller targets, postprocessor
  choices, input/output formats, dry-run evidence gates, blockers, required
  artifacts including robot frame/TCP and collision records, robotic extruder
  feedstock and purge records, robotic bead coupon and flow records, robotic cell
  interlock records, sheet-lamination stock/stack records, registration/trim
  records, bond/consolidation records, delamination/dimensional-release records,
  interlock and release records, inspection calibration records, datum alignment and
  uncertainty records, first-article measured-values reports, nonconformance
  disposition records, thermal profile and furnace logs, fixture/setter and
  atmosphere records, cooldown/quench and PPE records,
  distortion-hardness-release inspection records, assembly-kit travelers, robot-path or fixture simulation
  reports, final-fit metrology records, and operator signoff requirements;
  `controller-plan`, `parametric-design`, and `mdp-request` include
  `controllerPlan` compatibility targets, dialect summaries, postprocessor-known
  status, required controller checks, required evidence, controller release
  gates, blockers, and `controller-*` learning observations;
  `simulation-report`, `analysis-simulation-report`, and `mdp-request` include
  `riskProfile` program risk scores, high-risk program counts, recommended
  actions, and `simulation-risk:*` learning observations;
  `analysis-des-instruction-model` and `analysis-mdp-request` include
  `desInstructionModel` DES Studio review queues, per-program service-rate
  signals, machine-ready candidates, and structural analysis for imported
  instruction streams;
  `pomdp-belief-state`, `release-probe-plan`, `parametric-design`, and `mdp-request` include
  `pomdpBeliefState` hidden-state probabilities, observation likelihoods, and
  recommended probe actions for uncertain machine-failure, intervention,
  split/combine, automation, and program-valid states plus `releaseProbePlan`
  priority probes, release-blocker flags, and required-before-release actions; `learning-plan`,
  `neural-training-corpus`, and `mdp-request` include
  `neuralPolicy.engineInference`, DES `FeedForwardNetwork` parameter counts,
  output scores and top signal, plus `neuralTrainingCorpus` normalized training
  examples, feature vectors, labels, and strategy inference candidates;
  `outcome-remediation-plan` includes `outcomeRemediation` root causes,
  corrective actions, retry strategy, and learning signals from observed
  fabrication outcomes; `process-graph`, and
  `mdp-request` include `processGraph` operation nodes, dependencies, and
  release gates. `parametric-design` also embeds `designPackage`, `designExports`,
  `designInputReview`, `executionPlan`, `postprocessPlan`, `controllerPlan`, `releasePackagePlan`, `pomdpBeliefState`,
  `releaseProbePlan`,
  `machineRelease`, `manufacturingHandoff`, `productionPlan`, `machineSchedule`,
  `materialPlan`, `qualityPlan`, `toolingPlan`, `fixturePlan`,
  `monitoringPlan`, `interfaceControlPlan`, and `releasePackagePlan` for
  one-payload handoff review.

## Local Build

```bash
cd remote/deployments/fabrication-server-rs
cargo test
cargo run --release
```

The default local port is `8113`; set `PORT` to override it.
