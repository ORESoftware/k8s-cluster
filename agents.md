# Claritas data visualization server agent instructions

## Query, visualization, and security invariants

- Treat input parsing, semantic normalization, logical planning, SQL generation, dataset/schema metadata, chart/table output, and API responses as one coherent contract.
- SQL, DAX, and other expression frontends fail closed on unsupported syntax, comments, multiple statements, excessive nesting, invalid identifiers, unsafe characters, joins or constructs outside the documented subset, and ambiguous source selection.
- Preserve arithmetic precedence, expression-kind semantics, dependency extraction, row/filter context, aggregation behavior, grouping, limit clamping, warning generation, and normalized/compiled output.
- Never interpolate unvalidated user strings into SQL, HTML, XML, URLs, filenames, shell commands, or identifiers. Keep escaping and identifier-cleaning tests synchronized.
- Bound query rows, parsing depth, payload sizes, execution time, and error output. Never echo secrets or sensitive dataset contents through logs, errors, telemetry, or generated markup.
- Keep visualization output accessible, deterministic, structurally valid, and faithful to the logical plan. Do not hide unsupported semantics behind a misleading chart.

## Instruction discovery

Resolve `$PWD`, walk upward through every parent directory to the filesystem root, read every readable lowercase `agents.md` on that ancestor chain, and apply them root-to-leaf. Do not search siblings. Deduplicate resolved paths/inodes, avoid symlink cycles, and report unreadable files.

## Synchronize with the remote

Before editing, inspect `git status`, current branch, configured remotes, and the default branch. Run `git fetch --all --prune` and create the feature branch from the latest remote default branch. Fetch again before pushing and incorporate upstream changes with `git merge` or `git pull` on a clean working tree.

- avoid git rebase in favor of git merge.
- Never discard remote commits, force-push, rewrite shared history, bypass review, or bypass required CI.

## Resolve Git conflicts semantically

Resolve conflicts by understanding and combining both sides' intent. Do not mechanically choose `ours`, `theirs`, current, or incoming changes. Produce the conceptually correct result while preserving parser safety, logical semantics, normalization/SQL equivalence, bounds, escaping, warnings, output accessibility, tests, documentation, configuration, and public APIs. Regenerate snapshots or output fixtures from the merged implementation rather than selecting one side's generated output. If intentions are incompatible, make the smallest explicit design decision and document it in the pull request.

After resolving:

1. Reread every affected file from the top, not only conflict hunks.
2. Run formatting, linting, unit/integration tests, parser fuzz/property tests, API tests, and visualization snapshot/accessibility validation.
3. Search the entire worktree for conflict markers:

   ```sh
   grep -RInE '^(<<<<<<<|=======|>>>>>>>)' --exclude-dir=.git .
   ```

4. If any marker or suspicious partial resolution remains, repeat semantic resolution from the top and rerun validation.

A conflict is resolved only when the query and visualization pipeline is conceptually coherent and verified, not merely accepted by Git.