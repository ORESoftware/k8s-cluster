# Protected `mcp-rust-libs` publication

This runbook describes the trusted-main path that publishes the reviewed
`ORESoftware/testing@mcp-rust-libs/` subtree into the canonical public
`ORESoftware/mcp-rust-libs` repository.

## Security boundary

The GitHub Actions workflow does not carry a long-lived GitHub repository
administration token. It assumes the reviewed AWS role, transfers the exact
checked-in scripts and trusted `k8s-cluster` commit through SSM, and invokes a
root-owned broker on the protected host.

The root broker resolves one credential through the existing ordered sources:

1. AWS Secrets Manager through the instance role;
2. the reconciled Kubernetes Secret across the approved kubeconfigs;
3. the authenticated `ec2-user` GitHub CLI profile.

The broker validates the GitHub identity, base64-encodes the token, and passes
it only over stdin to the unprivileged child. The child rejects inherited
GitHub credentials and never discovers a credential by itself.

## Immutable publication inputs

The child publisher pins all of the following before any target mutation:

- source repository and commit;
- source subtree;
- source-manifest SHA-256;
- required polyglot package and workflow files;
- deterministic baseline and source commit timestamps;
- canonical target repository and bootstrap branch.

It runs the complete source regeneration, static, scaffold, conformance, and
manifest checks before creating the target repository.

## Repository history policy

Publication is create-only and no-force:

- an absent `main` receives the deterministic baseline commit;
- an existing `main` must have the exact reviewed baseline tree or the complete
  source tree;
- an existing bootstrap branch must have the exact reviewed source tree;
- any divergent tree fails closed and requires human semantic reconciliation;
- the implementation enters GitHub through an ordinary pull request and must
  pass the complete target polyglot matrix before merge.

Never resolve a divergent target by force-pushing or selecting one side of a
conflict. Compare the target history, the pinned source tree, and the latest
trusted-main publisher commits, then preserve all still-valid behavior in a
new reviewed branch.

## Verification after a run

A successful workflow log is not sufficient evidence. Verify directly:

1. `ORESoftware/mcp-rust-libs` exists and is public;
2. its default branch is `main`;
3. `main` equals the deterministic baseline commit until the bootstrap PR is
   merged, or equals the exact reviewed source tree afterward;
4. the bootstrap branch points at the publisher-reported head SHA;
5. exactly one open bootstrap pull request targets `main`;
6. the target workflow matrix is green on that exact head;
7. the merged target tree equals the pinned source subtree tree.

Record the workflow URL, reported source tree, target head SHA, pull request,
and final remote verification on the canonical Linear issues.

## Failure stages

The publisher emits `publisher-stage-failed=<stage> exit=<status>` without
printing credentials. Treat the stage as the first classification key:

- `receive-protected-credential`: broker-to-child handoff failed;
- `verify-github-identity`: token is absent, invalid, or belongs to another
  actor;
- `checkout-reviewed-source` or `validate-reviewed-source`: pinned carrier or
  source contract drifted;
- `ensure-target-repository`: repository administration failed;
- `prepare-target-review-gate`: deterministic baseline construction failed;
- `publish-reviewed-source-branch`: target history diverged or push failed;
- `ensure-target-pull-request`: the review gate could not be established.

Repair the earliest failed boundary and retrigger from trusted `main`. Do not
add token literals, broaden organization scope, disable validation, or weaken
the no-force/divergence checks.

## Local and CI checks

The focused suite is credential-free and mutation-free:

```bash
bash -n scripts/ops/publish_mcp_rust_libs.sh
bash -n scripts/ops/run_protected_mcp_rust_libs_publisher.sh
python3 scripts/ops/test_publish_mcp_rust_libs_contract.py
```

The `MCP Rust libraries publisher contracts` workflow runs the same checks on
all relevant pull requests and pushes to `main` with `contents: read` only.

References: DEN-319, DEN-957, DEN-959, DEN-967, DEN-968, DEN-969, DEN-970,
DEN-972, DEN-1186.
