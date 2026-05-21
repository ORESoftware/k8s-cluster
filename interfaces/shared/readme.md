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
│   └── runtime-config.schema.json
├── src/
│   └── generate.mjs              # JSON Schema → per-language types
├── generated/                    # check-in artifacts, never hand-edit
│   ├── typescript/
│   ├── rust/
│   ├── python/
│   └── gleam/
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
