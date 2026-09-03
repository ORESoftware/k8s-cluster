# Scintilla full-superproject multi-architecture benchmark

This repository owns the authoritative build context required by `scintilla-run/gleam-lambda-runner`. The runner cannot reproduce that context in its own pull-request workflow because `remote/libs` and the nested Scintilla repositories are private, while a repository-scoped `GITHUB_TOKEN` cannot read a different private repository.

`.github/workflows/scintilla-superproject-benchmark.yml` therefore performs the privileged proof here. It uses the existing `checkout-remote-libs` composite action with the narrowly scoped, read-only `K8S_LIBS_DEPLOY_KEY`, and uses the existing `ORG_GITOPS_TOKEN` secret only for the private Scintilla runner checkout. It never accepts a token as a workflow input, embeds a credential in a Git URL, or writes credentials to a report or artifact.

For each native architecture, the workflow:

1. resolves and checks out the exact `remote/libs` and Scintilla monorepo gitlinks;
2. overlays an explicitly selected immutable runner revision at the canonical nested path;
3. verifies the private library, monorepo-gitlink, and runner revisions independently;
4. builds the compatibility and lean images from the cluster-root context;
5. verifies the loaded OCI architecture;
6. starts fresh containers in alternating order and measures `docker run` through the first HTTP 200 from `/healthz`;
7. publishes JSON and Markdown evidence with image sizes, raw samples, median, and p95.

The pull request introducing this workflow benchmarks runner commit `fdb818fbd20ca6535d2915c50825dd87ba52d60f`. After merge, `workflow_dispatch` can benchmark another branch, tag, or immutable commit without changing the cluster gitlinks. The lean runner remains a draft until both architecture jobs complete successfully and the resulting evidence is reviewed.

These measurements isolate container-image startup on native x86_64 and arm64 GitHub-hosted machines. They are not AWS Lambda `Init Duration`; that separate measurement requires published Lambda versions and an AWS benchmark account, role, and log-retention policy.
