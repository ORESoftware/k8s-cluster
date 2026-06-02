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
