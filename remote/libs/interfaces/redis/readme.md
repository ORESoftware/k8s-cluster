# `@dd/redis-interfaces`

Cross-runtime source of truth for Redis key conventions and value shapes used by dd services.

JSON Schema (Draft 2020-12) is the canonical source of truth, with a small `$dd:redis`
extension block per file that declares **canonical key formatters**. The generator emits
idiomatic types and key-formatter functions for every runtime that consumes Redis (today:
TypeScript, Rust, Python, Gleam — adding a new language is one render function in
`src/generate.mjs`).

> Postgres lives in `remote/libs/pg-defs`. Generic cross-runtime interfaces (HTTP request
> bodies, control-plane payloads) live in `remote/libs/interfaces/shared`. This package is
> specifically for the **Redis layer**: which keys exist, how they are formatted, what
> value shape sits behind each key.

## Layout

```
remote/libs/interfaces/redis/
├── schema/                       # source of truth — JSON Schema + $dd:redis blocks
│   ├── index.json                # alphabetised list of every schema file
│   ├── agent-thread-breadcrumb-cache.schema.json
│   ├── container-pool-affinity-lock.schema.json
│   └── runtime-config-redis.schema.json
├── src/
│   └── generate.mjs              # JSON Schema → per-language types + key formatters
├── generated/                    # check-in artifacts, never hand-edit
│   ├── typescript/
│   ├── rust/
│   ├── python/
│   └── gleam/
└── package.json
```

## Generator

```bash
pnpm --filter @dd/redis-interfaces generate
pnpm --filter @dd/redis-interfaces check    # CI fail-if-stale
pnpm --filter @dd/redis-interfaces test     # generator unit tests
```

The generator handles the same JSON Schema subset as `@dd/shared-interfaces` for value
types, plus the `$dd:redis` extension for keys.

### `$dd:redis` extension

Each schema file MAY declare a `$dd:redis` block alongside its `$defs`:

```json
{
  "$dd:redis": {
    "service": "dd-runtime-config",
    "summary": "Redis is the storage backend for the runtime-config control plane.",
    "keys": [
      {
        "name": "RuntimeConfigEntryKey",
        "description": "JSON-encoded RuntimeConfigEntry from @dd/shared-interfaces.",
        "pattern": "{prefix}:{env}:entry:{scope}:{key}",
        "defaultPrefix": "dd:rc",
        "params": [
          { "name": "prefix", "type": "string", "description": "..." },
          { "name": "env",    "type": "string" },
          { "name": "scope",  "type": "string" },
          { "name": "key",    "type": "string" }
        ],
        "valueType": "json-shared-interface",
        "valueRef": "RuntimeConfigEntry"
      }
    ]
  }
}
```

Supported `valueType` markers (documentation only — Redis itself is untyped):

| `valueType`              | Meaning                                                                                  |
|--------------------------|------------------------------------------------------------------------------------------|
| `json`                   | A JSON-encoded value of the type in `valueRef`, where `valueRef` lives in `$defs`.      |
| `json-shared-interface`  | A JSON-encoded value of the type in `valueRef`, where the type lives in `@dd/shared-interfaces`. |
| `opaque-string`          | A plain ASCII string, no schema. (E.g. the lock token in `ContainerPoolAffinityLock`.) |
| `set-of-string`          | A Redis SET of plain strings.                                                            |
| `integer`                | A Redis INCR-style integer counter.                                                      |

`params[].type` is always `string` in v1; the generator emits string-typed parameters in
each language and uses string concatenation for the key.

## Consumers

### TypeScript (Node)

```ts
import {
  agentThreadBreadcrumbTailKey,
  AGENT_THREAD_BREADCRUMB_TAIL_KEY_DEFAULT_PREFIX,
  type AgentThreadBreadcrumb,
  type AgentThreadBreadcrumbTail,
} from '@dd/redis-interfaces/typescript';
```

### Rust

```toml
dd-redis-interfaces = { path = "../../libs/interfaces/redis/generated/rust" }
```

```rust
use dd_redis_interfaces::{
    agent_thread_breadcrumb_tail_key,
    AGENT_THREAD_BREADCRUMB_TAIL_KEY_DEFAULT_PREFIX,
    AgentThreadBreadcrumb, AgentThreadBreadcrumbTail,
};
```

### Python

```python
from dd_redis_interfaces import (
    agent_thread_breadcrumb_tail_key,
    AGENT_THREAD_BREADCRUMB_TAIL_KEY_DEFAULT_PREFIX,
    AgentThreadBreadcrumb,
    AgentThreadBreadcrumbTail,
)
```

### Gleam

```gleam
import dd_redis_interfaces.{
  agent_thread_breadcrumb_tail_key,
  agent_thread_breadcrumb_tail_key_default_prefix,
}
```

## Adding a new schema

1. Drop `schema/<name>.schema.json` with `$defs` (value types) and/or `$dd:redis` (keys).
2. Append the filename to `schema/index.json`.
3. Run `pnpm --filter @dd/redis-interfaces generate`.
4. Commit the regenerated outputs alongside the schema change.

CI runs the `check` script so untracked drift between schema and generated files fails the build.

## Why generated key formatters?

A misformatted Redis key is a silent cache-miss bug that crosses service boundaries. By
making every Redis key its own named function — emitted in every language that touches
Redis — services cannot drift on `dd:rc:prod:entry:foo:bar` vs `dd:rc:prod:entry:foo/bar`.
The function name is the only thing engineers refer to in code; the literal pattern lives
in exactly one place (the schema).
