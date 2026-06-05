# `@dd/shared-interfaces`

Cross-runtime shared types for dd services.

JSON Schema is the canonical source of truth. The generator emits idiomatic types for every
runtime that needs to consume the same payload (today: TypeScript, Rust, Python, Gleam — adding
new languages is one render function in `src/generate.mjs`).

## Layout

```
remote/libs/interfaces/shared/
├── schema/                       # source of truth — JSON Schema (Draft 2020-12)
│   ├── index.json                # alphabetised list of every schema file
│   ├── agent-orchestration.schema.json
│   ├── fabrication-cad-conversion.schema.json
│   └── runtime-config.schema.json
├── src/
│   └── generate.mjs              # JSON Schema → per-language types
├── generated/                    # check-in artifacts, never hand-edit
│   ├── typescript/
│   ├── rust/
│   ├── python/
│   └── gleam/
├── examples/                     # checked payload fixtures for worker contracts
└── package.json
```

## Generator

```bash
# regenerate every output
pnpm --filter @dd/shared-interfaces generate

# fail-if-stale check, run in CI
pnpm --filter @dd/shared-interfaces check
```

The generator handles a deliberately small subset of JSON Schema:

- Object types declared in `$defs` (one per file or grouped together).
- Primitive scalars: `string`, `integer`, `number`, `boolean`.
- Multi-type unions written as JSON-Schema arrays (e.g. `["string", "null"]` becomes nullable;
  unions that include `object` or `array` collapse to an opaque JSON value in every target
  language).
- Arrays via `items` (including arrays of `$ref`s).
- String enums via `enum`.
- Local `$ref` to `#/$defs/<Name>` (cross-document refs are not supported — keep related types
  in the same file).

If you find yourself wanting `oneOf`, polymorphism, or recursive types, prefer either a flatter
shape or a small Rust/TS type adapter rather than expanding the generator.

## Fabrication Design Synthesis

The fabrication design synthesis schema defines the JSON bodies published when the planner needs a
CAD-ready design candidate from intent, constraints, native CAD references, parametric templates, and
learning hints:

- `FabricationDesignSynthesisRequest` travels on
  `dd.remote.fabrication.design.synthesis.requests`.
- `FabricationDesignSynthesisResult` travels on
  `dd.remote.fabrication.design.synthesis.results`.

Those payloads carry object intent, dimensional and material constraints, design references,
templates, capabilities, learning hints, generated candidates, generated artifacts, retained blockers,
warnings, and review metadata. Native systems such as SOLIDWORKS, Creo/Pro/Engineer, Fusion, NX,
CATIA, and Onshape can be referenced through isolated synthesis/conversion workers, while FreeCAD,
OpenSCAD, Blender, and ZBrush sources use the same envelope when a worker can produce retained design
evidence.

The checked example pair in `examples/fabrication-design-synthesis-request.json` and
`examples/fabrication-design-synthesis-result.json` models a hybrid gearbox housing request that
selects a split printed shell plus milled bearing insert, emits OpenSCAD/STEP/3MF/assembly-graph
artifacts, and retains the machining/inspection blocker before downstream release.

## Fabrication Machine Profiles

The fabrication machine-profile schema defines JSON bodies published when the planner needs current
machine, tool, fixture, material, calibration, maintenance, and process-state evidence before design,
CAM, slicing, simulation, assembly planning, or release gates:

- `FabricationMachineProfileRequest` travels on
  `dd.remote.fabrication.machine.profiles.requests`.
- `FabricationMachineProfileResult` travels on
  `dd.remote.fabrication.machine.profiles.results`.

Those payloads carry requested evidence scopes, preferred machine ids, required machine classes,
capability snapshots, calibration/tool/fixture/material states, retained blockers, warnings, and
non-secret review metadata. The envelope covers additive printers, vertical and horizontal mills,
lathes, routers, laser/waterjet/plasma sheet cutters, postprocess stations, and inspection cells so
downstream workers can use live setup evidence without taking over final machine-ready authority.

The checked example pair in `examples/fabrication-machine-profile-request.json` and
`examples/fabrication-machine-profile-result.json` models a hybrid gearbox job that refreshes a Prusa
MK4, Haas VF2, Okuma lathe, waterjet, and CMM profile, then keeps machine-ready false for filament
conditioning, fixture proof, part-off support, and support-media verification.

## Fabrication CAD Conversion

The fabrication CAD conversion schema defines the JSON bodies published on the NATS subjects from
`@dd/nats-subject-defs`:

- `FabricationDesignConversionRequest` travels on
  `dd.remote.fabrication.design.conversion.requests`.
- `FabricationDesignConversionResult` travels on
  `dd.remote.fabrication.design.conversion.results`.

Those payloads carry reviewed `designInputs`, requested STEP/STL/3MF/DXF/CAM setup/sheet nesting
targets, sanitized source and artifact references, translator versions, generated neutral-export
artifacts, blockers, warnings, and non-secret review metadata. Native professional CAD systems such
as SOLIDWORKS, Creo/Pro/Engineer, NX, CATIA, Fusion, and Onshape stay behind isolated converter
workers; open/scriptable and artistic sources such as FreeCAD, OpenSCAD, Blender, and ZBrush use the
same envelope when a worker can produce verified manufacturing evidence.

The checked example pair in `examples/fabrication-design-conversion-request.json` and
`examples/fabrication-design-conversion-result.json` models a SOLIDWORKS native source request, STEP
and 3MF target exports, sanitized object references, translator version evidence, generated artifact
hashes, and the remaining machine-ready blocker that keeps final release with the Rust planner.

```bash
pnpm --filter @dd/shared-interfaces run validate:examples
```

That command validates the example pair against the checked schema, verifies request/result
correlation, rejects credential-bearing URIs, and confirms the result still carries a machine-ready
blocker for final planner release.

## Fabrication Instruction Generation

The fabrication instruction generation schema defines the JSON bodies published after the planner has
verified enough design evidence to ask slicer, CAM, postprocess, setup-sheet, inspection-plan, or
machine-code workers for manufacturing instructions:

- `FabricationInstructionGenerationRequest` travels on
  `dd.remote.fabrication.instructions.generation.requests`.
- `FabricationInstructionGenerationResult` travels on
  `dd.remote.fabrication.instructions.generation.results`.

Those payloads carry verified source artifacts, machine and controller profiles, additive,
subtractive, sheet-cutting, postprocess, inspection, or assembly operations, requested G-code/NC/setup
sheet/tool-list/simulation artifacts, generated instruction artifacts, preview lines, blockers,
warnings, and non-secret review metadata. The result envelope keeps `machineReady` under planner
control so workers can generate useful machine code while still surfacing workholding, toolpath,
material-conditioning, support-media, simulation, or human-intervention blockers.

The checked example pair in `examples/fabrication-instruction-generation-request.json` and
`examples/fabrication-instruction-generation-result.json` models a hybrid print-and-machine gearbox
housing job with a Prusa MK4 printer target, Haas VF2 mill target, generated printer G-code, generated
mill NC, a setup sheet, and remaining release blockers for workholding and filament conditioning.

`pnpm --filter @dd/shared-interfaces run validate:examples` validates design synthesis, machine
profiles, CAD conversion, instruction generation, and instruction simulation example pairs.

## Fabrication Instruction Simulation

The fabrication instruction simulation schema defines JSON bodies published when generated or
imported printer jobs, G-code, NC programs, lathe cycles, sheet-cutting files, and setup evidence need
machine-envelope, toolpath, thermal, material, workholding, support-media, controller-state, or
human-intervention verification:

- `FabricationInstructionSimulationRequest` travels on
  `dd.remote.fabrication.instructions.simulation.requests`.
- `FabricationInstructionSimulationResult` travels on
  `dd.remote.fabrication.instructions.simulation.results`.

Those payloads carry instruction artifacts, machine contexts, requested verification scopes, envelope
checks, simulation findings, failure boundaries, retained simulation artifacts, warnings, and review
metadata. The result keeps `machineReady` false whenever simulated instructions still need operator
proof, fixture models, material-conditioning evidence, part-off support, support media, or other
human-intervention evidence before release.

The checked example pair in `examples/fabrication-instruction-simulation-request.json` and
`examples/fabrication-instruction-simulation-result.json` models printer thermal/material checks,
Haas mill fixture-clearance/workholding checks, and Okuma lathe part-off support verification for the
gearbox job.

## Fabrication Instruction Review

The fabrication instruction review schema defines JSON bodies published when the planner receives
existing manufacturing instructions and needs workers to validate, boundary-label, or improve them
before release:

- `FabricationInstructionReviewRequest` travels on
  `dd.remote.fabrication.instructions.review.requests`.
- `FabricationInstructionReviewResult` travels on
  `dd.remote.fabrication.instructions.review.results`.

Those payloads carry submitted G-code, NC programs, slicer jobs, sheet-cutting files, setup sheets,
postprocess plans, and non-G-code shop instructions; requested review scopes; validation findings;
machine-failure boundaries; and safe improvement drafts. Review workers can propose patches or
rewrites, but the result keeps `machineReady` under planner control so imported instructions remain
blocked until workholding, thermal state, controller state, toolpath, support-media, material, and
human-intervention evidence is complete.

The checked example pair in `examples/fabrication-instruction-review-request.json` and
`examples/fabrication-instruction-review-result.json` models an imported Haas NC program and Prusa
G-code file with blocking workholding and hotend-wait findings plus improvement drafts that require
human approval.

`pnpm --filter @dd/shared-interfaces run validate:examples` validates design synthesis, machine
profiles, CAD conversion, instruction generation, instruction simulation, and instruction review
examples together.

## Fabrication Assembly Planning

The fabrication assembly planning schema defines the JSON bodies published before instruction
generation when the planner needs a worker to split, combine, join, sequence, or learn from a hybrid
manufacturing strategy:

- `FabricationAssemblyPlanningRequest` travels on
  `dd.remote.fabrication.assembly.planning.requests`.
- `FabricationAssemblyPlanningResult` travels on
  `dd.remote.fabrication.assembly.planning.results`.

Those payloads carry source artifacts, machine/process capabilities, candidate parts, join
interfaces, process steps, ranked plan candidates, MDP/POMDP learning signals, blockers, warnings,
and review metadata. This is the lane that lets workers recommend whether one requested object should
be split into printed, milled, turned, sheet-cut, or postprocessed pieces, or whether separate pieces
should be combined before machine-ready release.

The checked example pair in `examples/fabrication-assembly-planning-request.json` and
`examples/fabrication-assembly-planning-result.json` models a gearbox housing planner decision that
compares a monolithic print-and-machine route against a split printed shell, milled bearing insert,
and turned shaft route with heat-set inserts, datum-transfer inspection, and learning hints.

`pnpm --filter @dd/shared-interfaces run validate:examples` validates design synthesis, machine
profiles, CAD conversion, instruction generation, instruction simulation, instruction review,
assembly planning, learning outcome, and release-readiness examples together.

## Fabrication Learning Outcomes

The fabrication learning outcomes schema defines JSON bodies published after fabrication attempts,
simulations, inspections, or operator reviews produce evidence that should update MDP, POMDP,
neural-policy, reward, replay, or failure-boundary memory:

- `FabricationLearningOutcomeRequest` travels on
  `dd.remote.fabrication.learning.outcomes.requests`.
- `FabricationLearningOutcomeResult` travels on
  `dd.remote.fabrication.learning.outcomes.results`.

Those payloads carry source references, machine observations, human-intervention boundaries, reward
and penalty signals, retained failure boundaries, accepted replay updates, model or policy updates,
warnings, and review metadata. Learning remains explicit and auditable: a generated program or
assembly plan only becomes training evidence after an observed outcome records what succeeded, what
failed, and where the machine still needed human intervention or a split/combine plan change.

The checked example pair in `examples/fabrication-learning-outcome-request.json` and
`examples/fabrication-learning-outcome-result.json` models a gearbox run where the printed shell
succeeds, the mill cycle is blocked before start by missing workholding evidence, positive and
negative rewards are emitted, and replay/policy/failure-boundary memory accepts the updates.

`pnpm --filter @dd/shared-interfaces run validate:examples` validates design synthesis, machine
profiles, CAD conversion, instruction generation, instruction simulation, instruction review,
assembly planning, learning outcome, and release-readiness examples together.

## Fabrication Release Readiness

The fabrication release-readiness schema defines JSON bodies published at the final release gate,
after design synthesis, machine profiles, CAD conversion, instruction simulation, assembly planning,
generated or imported instruction review, and learning outcome evidence have been collected:

- `FabricationReleaseReadinessRequest` travels on
  `dd.remote.fabrication.release.readiness.requests`.
- `FabricationReleaseReadinessResult` travels on
  `dd.remote.fabrication.release.readiness.results`.

Those payloads carry evidence references, machine/process gates, requested release artifacts,
retained blockers, human-intervention requirements, release decisions, and final manifest artifacts.
The result keeps `machineReady` false whenever workholding, datum-transfer, material-conditioning,
inspection, operator-proof, or learning-boundary blockers still require human release evidence.

The checked example pair in `examples/fabrication-release-readiness-request.json` and
`examples/fabrication-release-readiness-result.json` models the gearbox split-plan release gate:
printer and mill artifacts are retained, but mill workholding proof, filament conditioning, and
datum-transfer inspection keep the selected hybrid plan blocked before machine-ready release.

## Consumers

### TypeScript (Node)

```ts
import type { RuntimeConfigApplyRequest } from '@dd/shared-interfaces/typescript';
```

The dev-server reads from `remote/libs/interfaces/shared/generated/typescript/index.ts` via a
relative path import so it stays inside the Docker context.

### Rust

The `generated/rust/` directory is a regular Cargo crate (`dd-shared-interfaces`). Add it as a
path dependency in each consuming `Cargo.toml`:

```toml
dd-shared-interfaces = { path = "../../libs/interfaces/shared/generated/rust" }
```

Then:

```rust
use dd_shared_interfaces::{RuntimeConfigApplyRequest, RuntimeConfigSnapshot};
```

### Python

```python
from dd_shared_interfaces import RuntimeConfigApplyRequest
```

The Python package lives at `generated/python/dd_shared_interfaces.py`; either add that
directory to `PYTHONPATH` or copy the file into the deployment's source tree at build time.

### Gleam

Add the path dependency to `gleam.toml`:

```toml
[dependencies]
dd_shared_interfaces = { path = "../../libs/interfaces/shared/generated/gleam" }
```

```gleam
import dd_shared_interfaces.{type RuntimeConfigApplyRequest}
```

## Adding a new schema

1. Drop a `schema/<name>.schema.json` file. Define the types under `$defs`.
2. Append the filename to `schema/index.json`.
3. Run `pnpm --filter @dd/shared-interfaces generate`.
4. Commit the regenerated files alongside the schema change.

## Adding a new target language

1. Add a `render<Lang>(model)` function in `src/generate.mjs`.
2. Wire it into `renderOutputs(model)` with an `add('generated/<lang>/...', render<Lang>(model))`.
3. Regenerate, eyeball the output, commit.

CI runs the `check` script so untracked drift between schema and generated files fails the build.
