# Zed package and git-submodule interop

`fiducia-monorepo` is an integration workspace and a Zed package. The two
mechanisms have different responsibilities and are deliberately kept
orthogonal:

- Git submodules pin complete repositories, including tools, deployment code,
  and integration fixtures.
- Zed dependencies expose reusable source/package edges to builds and agents.

The root package imports `fiducia-interfaces` and `fiducia-clients`. A future
`fiducia-lib` package belongs in the same dependency table after that
repository exists and is published. `fiducia-cli` and `fiducia-infra` may remain
submodules, but they are classified as tooling and operations and must never be
root Zed dependencies.

## Portable workflow

The supported workflow is identical on Linux, macOS, and Windows:

```sh
git clone --recurse-submodules https://github.com/fiducia-cloud/fiducia-monorepo.git
cd fiducia-monorepo
git submodule sync --recursive
git submodule update --init --recursive --jobs 4
python scripts/check-submodule-boundaries.py
```

The checker rejects absolute paths, parent-directory traversal, duplicate
paths or repositories, custom shell update commands, uninitialized nested
submodules, and CLI/infra package imports. CI repeats the recursive checkout and
validation on all three operating-system families.

When migrating a submodule to Zed, use the Zed git-submodule import/overtake
flow, commit the resulting manifest and lock changes, and keep the submodule
until consumers have moved. Do not remove a submodule merely because a Zed
coordinate exists; repository pinning and package resolution can coexist during
a staged migration.
