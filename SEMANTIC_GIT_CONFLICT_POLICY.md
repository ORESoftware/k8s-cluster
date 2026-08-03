# Semantic Git Conflict Resolution Policy

## Mandatory instruction

> resolve any and all git conflicts semantically, will full context, even looking back 3-10 commits in git log history for more context - never hastily pick sides in a conflict but merge things conceptually, using max context and complete conceptual awareness for a given github organization's repos and external org repos too

This instruction applies to human contributors, automation, AI agents, release tooling, and merge workflows.

Before resolving a conflict:

1. Identify the merge base and review both sides in context.
2. Inspect the relevant 3-10 commits with `git log`, `git show`, and `git blame` when useful.
3. Review related repositories in this organization and any external organizations that own affected APIs, schemas, packages, generated artifacts, infrastructure, or deployment contracts.
4. Preserve compatible intent and invariants from both sides. Do not make a purely textual selection.
5. Update related tests, documentation, schemas, lockfiles, migrations, generated code, CI, and deployment configuration.
6. Run the relevant validation and document the conceptual reasoning in the commit or pull request.
7. Surface unresolved ambiguity with evidence instead of guessing.

A resolution is complete only when conflict markers are gone, both sides were considered, cross-repository effects were checked, validation passes, and the reasoning is reviewable.
