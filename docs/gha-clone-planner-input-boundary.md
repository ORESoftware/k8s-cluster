# GHA clone planner input boundary

The independent clone-server lane accepts only a deliberately bounded static subset of GitHub workflow YAML. GitHub Actions and official Actions Runner Controller remain the native-semantics lane.

## Workflow identity

The planner accepts an exact repository identity, immutable revision for execution, and one direct workflow file named `.github/workflows/<file>.yml` or `.yaml`. Nested workflow paths, backslashes, non-YAML suffixes, and empty names fail before any fetch or plan is accepted.

## Raw YAML ambiguity checks

Before `serde_yaml` deserialization, the planner rejects tabs, YAML document markers, explicit tags, anchors, aliases, merge keys, duplicate block-mapping keys, and non-ASCII mapping keys. Each sequence item has its own mapping scope, so normal repeated step keys remain valid. Block-scalar command text is skipped by the structural scanner and is still evaluated by the fixed-profile compiler.

## Bounded limits

Workflow-byte, job-count, and per-job step limits must all be strictly positive. Zero is configuration failure, not an instruction to disable the bound.

These checks are defense in depth. They do not expand the independent lane into a general GitHub Actions interpreter and do not weaken the fail-closed fixed-profile boundary.
