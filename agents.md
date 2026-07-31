# Agent instructions

## Scope and hierarchy

- These instructions apply to the whole `zed-pkg/zed-monorepo` repository unless a deeper lowercase `agents.md` adds narrower rules.
- Before editing, resolve the current working directory and load every readable ancestor `agents.md` from the filesystem root to the working directory. Do not search siblings. Resolve symlinks, deduplicate resolved files, and report unreadable or cyclic instruction files.
- `.claude/CLAUDE.md`, `.gemini/GEMINI.md`, and `.openai/AGENTS.md` are pointers only. Never duplicate instructions in tool-specific files.

## Repository role

This repository is the pinned integration and submodule umbrella for the `zed-pkg` organization. It records exact cross-repository combinations under `apps/` and verifies them together. A submodule update is a release-set change, not a source edit to the vendored repository.

## Working rules

- Start from the current default branch and keep each change tied to its Linear issue.
- Inspect `README.md`, `.gitmodules`, `Makefile`, and existing workflows before changing pins or integration behavior.
- Preserve gitlink mode `160000`; do not replace submodules with copied directories or edit vendored contents as ordinary files.
- Update contract/interface pins before dependent services when a change spans repositories.
- Keep CI deterministic: pin external revisions, avoid mutable `main` checkouts inside tests, and do not add hidden network or credential requirements.
- Never commit tokens, kubeconfigs, registry credentials, generated secrets, or production environment files.
- Keep documentation, tests, and integration expectations synchronized with every release-set change.

## Validation

Run the focused repository checks documented in `README.md`. For instruction-policy changes also run:

```sh
python3 tools/agents_policy.py validate --repo . --print-chains
python3 -m unittest discover -s tests -p 'test_agents_policy.py' -v
```

The `agents policy` workflow is the authoritative cross-repository enforcement gate.
