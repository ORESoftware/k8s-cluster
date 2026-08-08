# GitHub ↔ Linear project sync checkpoint — 2026-08-07

This checkpoint records the current operating contract for the organization portfolio without duplicating the canonical registry. It was refreshed on 2026-08-08 after the current cross-organization delivery pass.

## Canonical sources

- Organization → Linear mapping: [`ops/portfolio/github-linear-project-registry.tsv`](./github-linear-project-registry.tsv).
- Human-readable organization directory: [`docs/portfolio/github-linear-projects-by-org.md`](../../docs/portfolio/github-linear-projects-by-org.md).
- Linear-side registry: `GitHub organization, Linear, and GitHub Project registry`.
- Repository, PR, commit, review, CI, release, and artifact truth comes from GitHub.
- Planning, ownership, dependency, milestone, and status truth comes from Linear.

## GitHub Project convention

For each organization in the canonical registry, the active organization project is named `<org>-project` and is normally available at `https://github.com/orgs/<org>/projects/1`.

The retained exception is `dancing-dragons`, whose canonical project is project `4`.

This document is a mapping/evidence checkpoint. It does **not** claim a GitHub Projects-v2 mutation when the executing integration did not perform one.

## Current coordinated execution

### ORESoftware / k8s-cluster

Canonical rollout ledger: `k8s-cluster#1111` / Linear DEN-845.

- `#1112` merged the initial immutable ChatGPT ↔ Claude bridge and Slack-command runtime into `dev`.
- `#1115` merged the initial digest-pinned provider runner at `replicas: 0`.
- `#1118` merged exporter watch-fallback parity plus focused runner-observability regression coverage.
- `#1119` merged the provider runner follow-up at source `c3e54e6cd0c6d56e3d2ed32902228d974e550a3f`, workflow run `31235992249`, digest `sha256:90a919fb28fb2bc2795a0a3735ab08993d245c3eaa2afcd5f42be9b1a4982702`, still held at zero replicas.
- `#1120` merged the bridge and signed Slack-command follow-up from the same source/run, using bridge digest `sha256:6b7e447a9989fa127ad4b0b3edc51fcd37a6b94a96bcf61b42c22d2641bf0ea8` and Slack-command digest `sha256:01f80fbd4d3ba5226b4abdb7f5e603538924edb48e79e72b0af43246624900cb`.
- Current rollout revision on `dev`: `c5f868b4598433d7ec5b3b96a853466ec89a9b49`.
- Slack remains `signed-dry-run`; the provider runner remains `held-zero`. No provider activation or spend was authorized by these merges.
- Focused bridge, Slack, runner, Kubernetes/kind, overlay, observability, E2E, OpenAPI, catalog, static, secret-scan, and no-PAT checks passed. The remaining broad repository failure is the pre-existing private-backend GitHub App installation-authority gate for unrelated private gitlinks; no PAT fallback was introduced.

Linear ownership remains segmented: DEN-845 owns immutable cluster rollout and live deployment evidence; DEN-391 owns the provider secret bundle; DEN-847 owns the bounded one-provider/one-replica canary; DEN-1041 owns end-user Slack/ChatGPT/Claude acceptance. DEN-845 must remain open until ArgoCD, ExternalSecret, exact live image-ID, probe, signed dry-run, authenticated transport, and digest-only rollback evidence are attached.

### meta-agents-demo

`meta-agents-demo/metagents#17` remains the current real-work introspection/UI candidate. The downstream `k8s-cluster#1114` GitOps activation remains dependency-gated and must not merge until the source PR is merged and its private image publication is proven.

### Portfolio automation

`k8s-cluster#1113` is the current nightly GitHub/Linear reconciliation candidate. It remains dependency-gated on the exact reviewed `ORESoftware/project-registry` source policy and must preserve the independent ChatGPT + Claude opinion contract before issue-state promotion.

## Cross-organization delivery delta — 2026-08-08

The canonical 64-organization directory was delivered through `ORESoftware/k8s-cluster#1056`. This section records delivery state only; it does not replace that generated mapping.

### athlet-o

- `athlet-o/athleto-sync#7` merged at reviewed head `b5756b27e39b0e780c4a3ce854725ddc09819be5`, producing squash commit `be828c251ebfdd0bf2f672beb573949efa7c5a34`.
- Linear DEN-1784 is Done. The event-listener upgrade and the advisory-specific, time-bounded RSA exception passed exact-head CI run `31240463205`.

### opto-sync and opto-sync-test

- `opto-sync/opto-sync-clients#73` merged from reviewed candidate `67a6912d0859e85532e9d72c379b6f2948803050` to squash commit `86b7192c6175697818d630b788705ac18a8198fa`.
- `opto-sync-test/background-wake-e2e#2` then certified that delivered commit and its provenance across browser, TypeScript, Dart/Flutter, Rust/WASM, Android, and Apple lanes before merging to `27ec384750e6cdfe6edff64067b850b3671e3709`.
- Linear DEN-2132 is Done.

### fiducia-cloud-test

- `fiducia-cloud-test/control-plane-e2e#2` merged at exact head `d0c680bd2ab797c7b1f579646458d9a406abd733` to squash commit `00aea547024eff8c654fb97a8f4f5f8b418757ae`.
- Linear DEN-2814 is Done for the deterministic file-lease fencing contract. The protected live lane remains separately opt-in and is not represented as production evidence.
- `fiducia-cloud-test/infra-multicloud-e2e#5` independently certified the read-only namespace Phase 0 source head used by `ORESoftware/k8s-cluster#1123`.

### ORESoftware namespace and independent-runner work

- `ORESoftware/k8s-cluster#1123` merged the read-only namespace ownership registry, deterministic 1,134-row migration manifest, strict ratchets, separate test-owner contract, and fail-closed execution boundary to `c70628646600ad8f2b88bb7f684fb4cba19626e1`.
- Linear DEN-2949 is Done. Parent DEN-2786 remains In Progress because classification cleanup, strict-mode activation, and the actual separately reviewed migration phases have not executed.
- `ORESoftware/k8s-cluster#1154` remains the active DEN-2805 carrier for independent-runner governance keys. It must not merge until its refreshed adversarial contracts and exact-head Rust suite are green.

### shared-auth and shared definitions

- `shared-auth/shared-auth-server.rs#48` is closed without merge and superseded by stacked PR `#50`; reopening the stale carrier is not part of the plan.
- Linear DEN-2994 owns operator-controlled SSH audience, scope, and TTL grants. Arbitrary caller-selected grants must fail closed before the SSH stack can merge.
- Linear DEN-2995 owns exact SQL enum labels in generated Rust serde contracts. The fix must be generator-first with deterministic regeneration and exact-label round-trip tests; no generated-only workaround is accepted.

## Conflict-resolution rule

When concurrent agent branches overlap:

1. inspect the latest default-branch behavior and both branch intents;
2. retain independent invariants from each side;
3. create an ordinary two-parent semantic merge when histories need reconciliation;
4. never choose `ours`/`theirs` wholesale, force-reset a reviewed branch, weaken a fail-closed check, or treat an unrelated baseline CI failure as product success;
5. update Linear with exact PR/SHA/evidence after the GitHub state changes.

## Delivery rule

Generated archives and chat artifacts are evidence only. A repository is considered delivered only when the GitHub repository exists, the intended default branch exists remotely, and the reviewed commit is reachable from that remote branch. Pull requests and project documentation should link those remote objects directly.
