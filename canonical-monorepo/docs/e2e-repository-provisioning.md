# Canonical E2E repository provisioning

`canonical-cloud` owns production source. `canonical-cloud-test` owns disposable integration fixtures, staging mirrors, and consumer projects used to validate exact source revisions before promotion.

The repository matrix is declared in `provisioning/canonical-e2e-repositories.json` and enforced by `scripts/provision_canonical_e2e.py`. The script is idempotent: it creates missing repositories, validates existing repository identity, and refuses to overwrite drifted managed files.

## Repository layout

The source organization receives one private orchestrator:

- `canonical-cloud/canonical-e2e`

The test organization receives a private API staging mirror plus scenario and language-consumer repositories:

- `canonical-cloud-test/canonical-api-server.rs`
- `canonical-cloud-test/api-server-contract-e2e`
- `canonical-cloud-test/monorepo-submodules-e2e`
- `canonical-cloud-test/zed-package-graph-e2e`
- `canonical-cloud-test/web-server-routing-e2e`
- `canonical-cloud-test/cli-install-e2e`
- `canonical-cloud-test/clients-rust-consumer`
- `canonical-cloud-test/clients-typescript-consumer`
- `canonical-cloud-test/clients-go-consumer`
- `canonical-cloud-test/clients-python-consumer`
- `canonical-cloud-test/mcp-contract-e2e`
- `canonical-cloud-test/legacy-mirror-guard-e2e`

Reusable orchestrator, scenario, and consumer repositories are initialized as Zed packages with `.zpkg.toml` and `.zpkg.lock`. The `canonical-api-server.rs` staging mirror is intentionally empty and non-Zed because it receives an exact tested source revision rather than becoming another package or source of truth.

## Immutable-source contract

Every initialized test repository receives `.canonical-source-contract.json`, a README boundary, and a pinned repository-contract workflow. The contract requires:

- production source remains in `canonical-cloud`;
- test runs identify the source repository and immutable commit SHA, release tag, image digest, or artifact digest;
- promotion reuses the tested SHA or digest;
- test repositories never become independent production source;
- generated Zed identity matches the GitHub owner and repository name.

`canonical.cloud` remains a compatibility mirror. Active integration belongs in `canonical-monorepo`, whose source gitlinks live directly under `apps/`. `canonical-api-server.rs` should be added as `apps/canonical-api-server.rs` only after its exact PR head passes the test-organization staging gate, merges, and a reviewed `main` SHA is available.

## Provisioner GitHub App

Create or reuse a dedicated GitHub App and install it on both `canonical-cloud` and `canonical-cloud-test` with **All repositories** selected so newly created repositories remain accessible. Grant only these repository permissions:

- Administration: read and write
- Contents: read and write
- Workflows: write

Administration is required to create and configure repositories. Contents and Workflows are required to seed the managed files, including the pinned contract workflow.

Configure these values in `canonical-cloud/canonical-monorepo`:

- repository variable `CANONICAL_REPO_PROVISIONER_CLIENT_ID`
- repository secret `CANONICAL_REPO_PROVISIONER_PRIVATE_KEY`
- protected environment `canonical-repository-provisioning`, with required reviewers and restricted deployment branches
- environment secret `CANONICAL_REPOSITORY_PROVISIONING_APPROVED` set to `canonical-e2e-repository-provisioning-approved`

The workflow creates short-lived installation tokens independently for each organization and narrows them to read permissions for planning or write permissions for the protected apply job. The apply job verifies the environment-only approval marker before requesting either write token, so an absent or unconfigured environment fails closed.

## Operation

First run **provision-canonical-e2e** with `mode=plan`. The plan performs no writes and prints which repositories are absent.

After reviewing the plan, run it with `mode=apply` through the protected environment. The required confirmation is deliberately exact:

```text
provision canonical-cloud/canonical-e2e and canonical-cloud-test
```

The apply step is safe to rerun. Existing repositories are validated, matching managed files are left unchanged, and unexpected managed-file drift fails closed.

## Follow-up controls

After provisioning:

1. Add organization rulesets or branch protection requiring the repository-contract workflow.
2. Configure the staging credential consumed by the opt-in `canonical-cloud-test` workflow.
3. Run the API server's exact PR head in the staging mirror.
4. Merge the API only after source CI and the test-organization gate both pass.
5. Pin the reviewed API `main` SHA beneath `canonical-monorepo/apps/`.
6. Expand `canonical-e2e` to dispatch and aggregate all scenario repositories by immutable revision.
