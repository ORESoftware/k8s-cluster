# Future Work

## Submodule Branch Tracking

Git supports `submodule.<name>.branch = .`, meaning "use the same branch name as
the current superproject branch" for submodule remote updates. See the
[Git submodule docs](https://git-scm.com/docs/git-submodule).

That could make feature-branch workflows cleaner than rewriting every
`.gitmodules` entry to `feature/foo`. For release pins, explicit `main`/`dev`
is still clearer.
