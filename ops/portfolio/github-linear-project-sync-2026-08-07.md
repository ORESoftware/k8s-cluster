# GitHub ↔ Linear project sync checkpoint — 2026-08-07

This checkpoint records the current operating contract for the organization portfolio without duplicating the canonical registry.

## Canonical sources

- Organization → Linear mapping: [`ops/portfolio/github-linear-project-registry.tsv`](./github-linear-project-registry.tsv).
- Linear-side registry: `GitHub organization, Linear, and GitHub Project registry`.
- Repository, PR, commit, review, CI, release, and artifact truth comes from GitHub.
- Planning, ownership, dependency, milestone, and status truth comes from Linear.

## GitHub Project convention

For each organization in the canonical registry, the active organization project is named `<org>-project` and is normally available at `https://github.com/orgs/<org>/projects/1`.

The retained exception is `dancing-dragons`, whose canonical project is project `4`.

This document is a mapping/evidence checkpoint. It does **not** claim a GitHub Projects-v2 mutation when the executing integration did not perform one.

## Current coordinated execution

### ORESoftware / k8s-cluster

- `#1112` merged the immutable ChatGPT ↔ Claude bridge and Slack-command runtime into `dev`.
- `#1115` merged the digest-pinned provider runner at `replicas: 0`.
- `#1118` merged exporter watch-fallback parity plus focused runner-observability regression coverage.
- `#1119` is the current runner-release repin. Its branch is semantically synchronized with `dev` through a two-parent merge so the newer observability fix and the reviewed current runner release are both preserved.
- The remaining broad repository failure is the existing private-backend installation-authority gate; product-specific runner, observability, E2E, secret, no-PAT, catalog, and overlay contracts are evaluated separately.

Linear ownership: DEN-845 for immutable cluster rollout; DEN-847 for the bounded one-provider/one-replica canary; DEN-391 for the provider secret bundle. The runner remains held at zero until those activation gates are satisfied.

### meta-agents-demo

`meta-agents-demo/metagents#17` remains the current real-work introspection/UI candidate. The downstream `k8s-cluster#1114` GitOps activation remains dependency-gated and must not merge until the source PR is merged and its private image publication is proven.

### Portfolio automation

`k8s-cluster#1113` is the current nightly GitHub/Linear reconciliation candidate. It remains dependency-gated on the exact reviewed `ORESoftware/project-registry` source policy and must preserve the independent ChatGPT + Claude opinion contract before issue-state promotion.

## Conflict-resolution rule

When concurrent agent branches overlap:

1. inspect the latest default-branch behavior and both branch intents;
2. retain independent invariants from each side;
3. create an ordinary two-parent semantic merge when histories need reconciliation;
4. never choose `ours`/`theirs` wholesale, force-reset a reviewed branch, weaken a fail-closed check, or treat an unrelated baseline CI failure as product success;
5. update Linear with exact PR/SHA/evidence after the GitHub state changes.

## Delivery rule

Generated archives and chat artifacts are evidence only. A repository is considered delivered only when the GitHub repository exists, the intended default branch exists remotely, and the reviewed commit is reachable from that remote branch. Pull requests and project documentation should link those remote objects directly.
