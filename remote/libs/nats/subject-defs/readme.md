# `@dd/nats-subject-defs`

Cross-runtime source of truth for **NATS subject names** used across dd services.

NATS subjects come in two flavours in our system:

1. **Static** — a fixed string on a channel/topic, e.g. `dd.remote.events`,
   `dd.remote.orchestrator.wakeup`. These behave like enum values: a small
   closed set of names that producers publish to and consumers subscribe to.
2. **Parameterized** — a pattern that gets a value (user id, conv id, thread
   id, function name, pool slug, …) interpolated at runtime, e.g.
   `dd.remote.thread.{thread_id}.tasks`. Consumers typically subscribe to a
   wildcard form (`dd.remote.thread.*.tasks`) and extract the dynamic token
   from the resolved subject when a message arrives.

This package emits **constants** for static subjects and both **formatter +
parser** functions for parameterized subjects in every language that talks to
NATS in this repo. That removes the magic strings from each deployment and
lets producer and consumer code reference the same name without drift.

> Postgres lives in `remote/libs/pg-defs`. Redis lives in
> `remote/libs/interfaces/redis`. Generic cross-runtime payload shapes live in
> `remote/libs/interfaces/shared`. This package is specifically for the
> **NATS subject layer**: which subjects exist, what their wildcard form is,
> which JetStream stream they belong to, and which queue group consumers use.
> Payload shape definitions stay in `@dd/shared-interfaces`.

## Layout

```
remote/libs/nats/subject-defs/
├── schema/                       # source of truth — JSON Schema + $dd:nats blocks
│   ├── index.json                # alphabetised list of every schema file
│   ├── agent-orchestration.schema.json
│   ├── ai-ml-platform.schema.json
│   ├── container-pool.schema.json
│   ├── contracts.schema.json
│   ├── fabrication.schema.json
│   ├── lambdas.schema.json
│   ├── presence.schema.json
│   ├── runtime-events.schema.json
│   ├── trading.schema.json
│   └── wal-cdc.schema.json
├── src/
│   ├── generate.mjs              # JSON Schema → per-language outputs
│   └── generate.test.mjs         # drift check + formatter/parser round-trip
├── generated/                    # check-in artifacts, never hand-edit
│   ├── typescript/
│   ├── rust/
│   ├── python/
│   ├── gleam/
│   ├── erlang/
│   ├── dart/
│   ├── go/
│   └── jvm/
└── package.json
```

## Generator

```bash
pnpm --filter @dd/nats-subject-defs generate
pnpm --filter @dd/nats-subject-defs check    # CI fail-if-stale
pnpm --filter @dd/nats-subject-defs test     # generator unit tests
```

### `$dd:nats` extension

Each schema file declares a `$dd:nats` block with three top-level lists
(`subjects`, optional `queueGroups`, optional `streams`):

```json
{
  "$dd:nats": {
    "service": "dd-remote-rest-api",
    "summary": "Per-thread task dispatch and orchestrator wakeup signals.",
    "subjects": [
      {
        "name": "ThreadTasks",
        "description": "Per-thread task queue. JetStream-backed.",
        "kind": "parameterized",
        "pattern": "dd.remote.thread.{thread_id}.tasks",
        "wildcard": "dd.remote.thread.*.tasks",
        "params": [
          { "name": "thread_id", "type": "string", "description": "Thread UUID." }
        ],
        "direction": "both",
        "queueGroup": "dd-remote-thread-preparer",
        "stream": "DD_REMOTE_TASKS"
      },
      {
        "name": "OrchestratorWakeup",
        "description": "Wakeup signal for the orchestrator.",
        "kind": "static",
        "subject": "dd.remote.orchestrator.wakeup",
        "direction": "both",
        "stream": "DD_REMOTE_CONTROL"
      }
    ],
    "queueGroups": [
      {
        "name": "ThreadPreparerQueueGroup",
        "value": "dd-remote-thread-preparer",
        "description": "Shared queue group for thread-preparer consumers."
      }
    ],
    "streams": [
      {
        "name": "DD_REMOTE_TASKS",
        "description": "JetStream file storage, explicit ack, dedupe by Nats-Msg-Id.",
        "subjects": ["dd.remote.thread.*.tasks"],
        "retention": "limits",
        "storage": "file",
        "ack": "explicit"
      }
    ]
  }
}
```

#### Subject fields

| Field | Required | Meaning |
|---|---|---|
| `name` | yes | PascalCase identifier used to derive constant + function names per language. |
| `description` | recommended | Human-readable; emitted as docstring/comment. |
| `kind` | yes | `static` or `parameterized`. |
| `subject` | required for `static` | The literal NATS subject string. |
| `pattern` | required for `parameterized` | Pattern with `{param}` placeholders. |
| `wildcard` | required for `parameterized` | The wildcard form consumers subscribe to (`*` for tokens, `>` for tail). |
| `params` | required for `parameterized` | Ordered list of `{ name, type, description }`. Only `string` is supported in v1. |
| `direction` | optional | `publish`, `subscribe`, or `both`. Documentation only. |
| `queueGroup` | optional | Default NATS queue group used by consumers. |
| `stream` | optional | Name of the JetStream stream the subject is bound to. |

#### Wildcard rules

- `*` matches exactly one token.
- `>` matches one or more remaining tokens (tail wildcard).
- The generated parser walks the pattern token-by-token. A `{param}`
  placeholder must align with exactly one resolved token; literal tokens must
  match exactly. If the pattern ends with a `>` placeholder
  (`{param>}`), the generator captures the entire tail joined back with `.`.

## Consumers

The same names appear in every language; only the casing convention changes.

### TypeScript (Node)

```ts
import {
  DD_REMOTE_EVENTS_SUBJECT,                 // static enum-style constant
  RUNTIME_CRITICAL_EVENTS_SUBJECT,          // alert-worthy runtime failures
  threadTasksSubject,                       // formatter for publish
  parseThreadTasksSubject,                  // parser for subscriber
  THREAD_TASKS_WILDCARD,                    // subscribe target
  THREAD_TASKS_QUEUE_GROUP,                 // queue group constant
  DD_REMOTE_TASKS_STREAM_NAME,              // stream constant
  DD_REMOTE_TASKS_STREAM_SUBJECTS,
} from '@dd/nats-subject-defs/typescript';

nats.publish(threadTasksSubject(threadId), payload);
const sub = nats.subscribe(THREAD_TASKS_WILDCARD, { queue: THREAD_TASKS_QUEUE_GROUP });
for await (const m of sub) {
  const parsed = parseThreadTasksSubject(m.subject);
  if (parsed) handle(parsed.thread_id, m.data);
}
```

### Rust

```toml
dd-nats-subject-defs = { path = "../../libs/nats/subject-defs/generated/rust" }
```

```rust
use dd_nats_subject_defs::{
    DD_REMOTE_EVENTS_SUBJECT, RUNTIME_CRITICAL_EVENTS_SUBJECT,
    DD_REMOTE_CRITICAL_EVENTS_STREAM_NAME,
    thread_tasks_subject, parse_thread_tasks_subject,
    THREAD_TASKS_WILDCARD, THREAD_TASKS_QUEUE_GROUP,
    DD_REMOTE_TASKS_STREAM_NAME,
};
```

`RUNTIME_CRITICAL_EVENTS_SUBJECT` is backed by `DD_REMOTE_CRITICAL_EVENTS_STREAM_NAME`; the
queue-consumer deployment uses that durable stream to log/alert on compact critical runtime events.

### Fabrication Machine Profiles

Machine-profile workers consume printer, mill, lathe, router, sheet-cutter, postprocess, and
inspection inventory requests from `FABRICATION_MACHINE_PROFILE_REQUESTS_SUBJECT`
(`dd.remote.fabrication.machine.profiles.requests`) with queue group
`FABRICATION_MACHINE_PROFILE_REQUESTS_QUEUE_GROUP`
(`dd-fabrication-machine-profilers`). Workers publish capability snapshots, calibration state,
tool/fixture readiness, material and process state, maintenance blockers, and release evidence on
`FABRICATION_MACHINE_PROFILE_RESULTS_SUBJECT`
(`dd.remote.fabrication.machine.profiles.results`). This gives design synthesis, instruction
generation, simulation, and release readiness a typed source of current machine capability and
setup evidence.

### Fabrication Design Synthesis

Design-synthesis workers consume fabrication intent, dimensions, constraints, parametric templates,
native CAD references, and learning hints from `FABRICATION_DESIGN_SYNTHESIS_REQUESTS_SUBJECT`
(`dd.remote.fabrication.design.synthesis.requests`) with queue group
`FABRICATION_DESIGN_SYNTHESIS_REQUESTS_QUEUE_GROUP`
(`dd-fabrication-design-synthesizers`). Workers publish generated design candidates, parametric
source artifacts, manufacturability evidence, blockers, and review metadata on
`FABRICATION_DESIGN_SYNTHESIS_RESULTS_SUBJECT`
(`dd.remote.fabrication.design.synthesis.results`). This gives the Rust planner a typed lane for
turning a requested object into CAD-ready candidates before conversion, CAM, slicing, or assembly
decomposition.

### Fabrication CAD Conversion

The fabrication planner publishes native CAD, mesh, slicer, CAM setup, and neutral-export conversion
work on `FABRICATION_DESIGN_CONVERSION_REQUESTS_SUBJECT`
(`dd.remote.fabrication.design.conversion.requests`) with queue group
`FABRICATION_DESIGN_CONVERSION_REQUESTS_QUEUE_GROUP`
(`dd-fabrication-design-converters`). External converter workers publish translator evidence,
neutral exports, blockers, and review metadata on
`FABRICATION_DESIGN_CONVERSION_RESULTS_SUBJECT`
(`dd.remote.fabrication.design.conversion.results`). This keeps SOLIDWORKS, Creo/Pro/Engineer, NX,
CATIA, Fusion, Onshape, FreeCAD, OpenSCAD, Blender, ZBrush, slicer, and CAM conversion work out of
the core Rust planner while preserving stable cross-language subject constants.

### Fabrication Instruction Generation

Instruction-generation workers consume verified geometry, machine profiles, operation plans, and
release evidence from `FABRICATION_INSTRUCTION_GENERATION_REQUESTS_SUBJECT`
(`dd.remote.fabrication.instructions.generation.requests`) with queue group
`FABRICATION_INSTRUCTION_GENERATION_REQUESTS_QUEUE_GROUP`
(`dd-fabrication-instruction-generators`). Slicer, CAM, postprocess, setup-sheet, tool-list,
inspection-plan, and controller-specific adapters publish machine-code and operator-instruction
evidence on `FABRICATION_INSTRUCTION_GENERATION_RESULTS_SUBJECT`
(`dd.remote.fabrication.instructions.generation.results`). The Rust planner retains final
machine-ready authority so generated G-code, NC programs, lathe cycles, setup sheets, and printer
jobs can still surface simulation, workholding, material-conditioning, toolpath, or human-intervention
blockers before release.

### Fabrication Instruction Simulation

Simulation workers consume generated or imported printer jobs, G-code, NC programs, lathe cycles,
sheet-cutting files, setup sheets, and machine profiles from
`FABRICATION_INSTRUCTION_SIMULATION_REQUESTS_SUBJECT`
(`dd.remote.fabrication.instructions.simulation.requests`) with queue group
`FABRICATION_INSTRUCTION_SIMULATION_REQUESTS_QUEUE_GROUP`
(`dd-fabrication-instruction-simulators`). Workers publish machine-envelope checks, toolpath and
process findings, failure boundaries, retained release blockers, and simulation artifacts on
`FABRICATION_INSTRUCTION_SIMULATION_RESULTS_SUBJECT`
(`dd.remote.fabrication.instructions.simulation.results`). This gives the planner a typed verification
lane before release readiness for printer thermal/material state, CNC workholding and toolpath
geometry, sheet-cutting support media, lathe part-off support, and human-intervention boundaries.

### Fabrication Instruction Review

Instruction-review workers consume imported controller programs, printer jobs, slicer projects,
sheet-cutting files, setup sheets, and operator instructions from
`FABRICATION_INSTRUCTION_REVIEW_REQUESTS_SUBJECT`
(`dd.remote.fabrication.instructions.review.requests`) with queue group
`FABRICATION_INSTRUCTION_REVIEW_REQUESTS_QUEUE_GROUP`
(`dd-fabrication-instruction-reviewers`). Reviewers publish validation findings, improvement drafts,
machine-failure boundaries, and remaining release blockers on
`FABRICATION_INSTRUCTION_REVIEW_RESULTS_SUBJECT`
(`dd.remote.fabrication.instructions.review.results`). This keeps submitted G-code, NC programs,
lathe cycles, printer jobs, waterjet/laser/plasma files, setup sheets, and non-G-code shop
instructions auditable before the planner accepts, improves, or releases them.

### Fabrication Assembly Planning

Hybrid assembly planners consume object intent, candidate source artifacts, machine capabilities,
and process constraints from `FABRICATION_ASSEMBLY_PLANNING_REQUESTS_SUBJECT`
(`dd.remote.fabrication.assembly.planning.requests`) with queue group
`FABRICATION_ASSEMBLY_PLANNING_REQUESTS_QUEUE_GROUP`
(`dd-fabrication-assembly-planners`). Workers publish part decomposition, combine/split decisions,
join interfaces, process sequencing, MDP/POMDP learning-state hints, and remaining release blockers
on `FABRICATION_ASSEMBLY_PLANNING_RESULTS_SUBJECT`
(`dd.remote.fabrication.assembly.planning.results`). This gives the Rust planner a typed lane for
deciding when one object should become several printed, milled, turned, sheet-cut, or postprocessed
pieces, or when separate pieces should be combined before machine-ready release.

### Fabrication Execution Telemetry

Execution telemetry reviewers consume observed printer, mill, lathe, router, sheet-cutting,
assembly, inspection, and postprocess run results from
`FABRICATION_EXECUTION_TELEMETRY_REQUESTS_SUBJECT`
(`dd.remote.fabrication.execution.telemetry.requests`) with queue group
`FABRICATION_EXECUTION_TELEMETRY_REQUESTS_QUEUE_GROUP`
(`dd-fabrication-execution-reviewers`). Workers publish run state, machine stops, operator
interventions, split/combine or redesign decisions, retained artifacts, and learning labels on
`FABRICATION_EXECUTION_TELEMETRY_RESULTS_SUBJECT`
(`dd.remote.fabrication.execution.telemetry.results`). This gives release readiness and learning
workers a typed evidence lane after hardware execution so stopped or partially completed jobs can be
reviewed before repeat release and converted into MDP/POMDP/neural outcome signals.

### Fabrication Learning Outcomes

Learning updaters consume completed job outcomes, machine observations, reward hints, failure
boundaries, and human-intervention evidence from `FABRICATION_LEARNING_OUTCOME_REQUESTS_SUBJECT`
(`dd.remote.fabrication.learning.outcomes.requests`) with queue group
`FABRICATION_LEARNING_OUTCOME_REQUESTS_QUEUE_GROUP`
(`dd-fabrication-learning-updaters`). MDP/POMDP, neural-policy, replay-buffer, reward-model, and
failure-boundary workers publish accepted learning updates on
`FABRICATION_LEARNING_OUTCOME_RESULTS_SUBJECT`
(`dd.remote.fabrication.learning.outcomes.results`). This keeps outcome learning explicit and
auditable: generated instructions, assembly plans, and operator interventions only become training
signals after the planner records the observed result and retained boundaries.

### Fabrication Release Readiness

Release-readiness workers consume final evidence, machine gates, retained blockers, requested
artifacts, and human-intervention state from `FABRICATION_RELEASE_READINESS_REQUESTS_SUBJECT`
(`dd.remote.fabrication.release.readiness.requests`) with queue group
`FABRICATION_RELEASE_READINESS_REQUESTS_QUEUE_GROUP`
(`dd-fabrication-release-gates`). Workers publish machine-ready decisions, release manifests,
required human interventions, and retained blockers on
`FABRICATION_RELEASE_READINESS_RESULTS_SUBJECT`
(`dd.remote.fabrication.release.readiness.results`). This keeps the Rust planner as the final
machine-ready authority while giving machine-profile, design synthesis, CAD conversion, assembly
planning, instruction generation, instruction simulation, instruction review, and learning workers a
typed evidence bundle for the last release gate.

### Python

```python
from dd_nats_subject_defs import (
    DD_REMOTE_EVENTS_SUBJECT,
    thread_tasks_subject, parse_thread_tasks_subject,
    THREAD_TASKS_WILDCARD, THREAD_TASKS_QUEUE_GROUP,
)
```

### Gleam

```gleam
import dd_nats_subject_defs.{
  dd_remote_events_subject,
  thread_tasks_subject, parse_thread_tasks_subject,
  thread_tasks_wildcard, thread_tasks_queue_group,
}
```

### Erlang

```erlang
Subject = dd_nats_subject_defs:thread_tasks_subject(ThreadId),
case dd_nats_subject_defs:parse_thread_tasks_subject(Received) of
    {ok, #{thread_id := ThreadId}} -> handle(ThreadId);
    error -> drop
end,
QueueGroup = dd_nats_subject_defs:thread_tasks_queue_group(),
RuntimeEvents = dd_nats_subject_defs:dd_remote_events_subject().
```

### Dart

```dart
import 'package:dd_nats_subject_defs/dd_nats_subject_defs.dart';

nats.publish(threadTasksSubject(threadId), payload);
final parsed = parseThreadTasksSubject(received);
```

### Go

```go
import ddnats "dd/nats/subject-defs/generated/go"

nc.Publish(ddnats.ThreadTasksSubject(threadID), payload)
parsed, ok := ddnats.ParseThreadTasksSubject(received)
```

### Java 17+

```java
import dd.nats.DdNatsSubjects;

connection.publish(DdNatsSubjects.threadTasksSubject(threadId), payload);
var parsed = DdNatsSubjects.parseThreadTasksSubject(received);
parsed.ifPresent(p -> handle(p.threadId()));
```

## Adding a new subject

1. Pick the right `schema/<name>.schema.json` file (or add a new one + list it
   in `schema/index.json`).
2. Append a new entry to the `$dd:nats.subjects` array.
3. Run `pnpm --filter @dd/nats-subject-defs generate`.
4. Commit the regenerated outputs alongside the schema change.

CI runs the `check` script so untracked drift between schema and generated
files fails the build.

## Why generated subject definitions?

A subscriber typo on `dd.remote.thread.*.tasks` is a silent zero-delivery bug
that crosses language boundaries — Rust producer, Erlang consumer, Python
analyzer, Dart load-test client. By making every subject (and every wildcard,
queue group, and JetStream stream) its own named function or constant emitted
in every language that touches NATS, services cannot drift on
`dd.remote.thread.{id}.tasks` vs `dd.remote.thread.{id}.task` or on
`presence.broadcast.conv.{id}` vs `presence.broadcasts.conv.{id}`. The
function name is the only thing engineers refer to in code; the literal
subject pattern lives in exactly one place (the schema file).
