# GHA clone planner input boundary

The independent clone-server lane accepts only a deliberately bounded static subset of GitHub workflow YAML. GitHub-hosted runners and official Actions Runner Controller remain the native-semantics lanes.

## Workflow identity

Planning accepts one exact repository identity, an immutable revision for execution, and one direct ASCII workflow file named `.github/workflows/<file>.yml` or `.yaml`. Nested paths, backslashes, repeated-dot traversal forms, non-YAML suffixes, whitespace, non-ASCII names, and empty names fail before a plan is accepted.

## Raw YAML ambiguity checks

Before `serde_yaml` deserialization, the planner rejects tabs, YAML document markers, tags/anchors/aliases in structural positions, merge keys, duplicate block-mapping keys, non-ASCII mapping keys, and non-empty flow mappings. Each sequence item receives its own mapping scope, while duplicate keys inside one item fail closed.

Block-scalar command text is excluded from structural scanning. The scanner uses the effective mapping-key column for `- run: |`, so sibling fields return to the same sequence-item scope and cannot hide duplicate keys after a script body.

## Bounded limits

Workflow-byte, job-count, and per-job step limits must all be strictly positive. Zero is configuration failure, not an instruction to disable the bound.

## Validation invariant

The all-target Rust suite covers valid repeated mapping keys across separate steps, duplicate keys inside one step, sibling fields after `- run: |`, block-scalar text containing YAML-looking tokens, document-marker comments, non-empty flow mappings, direct-path character constraints, and zero planner limits. The stricter path validator retains the established `workflowPath must stay under .github/workflows` diagnostic prefix so existing operators and adversarial contract tests remain compatible.

Formatting, warnings-denied Clippy, library and binary unit tests, HTTP/process integration tests, router assignment tests, meta self-tests, and planner adversarial tests must all pass before this boundary changes.

## Current-dev proof

The product tree was merged with and validated against exact `dev` commit `208bdcbe00f17a2a4a17548b28fe7a563a66445e`. The validation merge was accepted only after the final tree differed from that base in this runbook and `remote/deployments/gha-clone-server-rs/src/lib.rs`, with no temporary workflow remaining.

These checks are defense in depth. They do not expand the independent lane into a general GitHub Actions interpreter and do not weaken the fixed-profile compiler's fail-closed boundary.
