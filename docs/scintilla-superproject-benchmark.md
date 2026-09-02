# Scintilla full-superproject multi-architecture benchmark

This repository owns the authoritative build context required by `scintilla-run/gleam-lambda-runner`. The runner cannot reproduce that context in its own pull-request workflow because `remote/libs` is a private gitlink and a repository-scoped `GITHUB_TOKEN` cannot read a different private repository.

`.github/workflows/scintilla-superproject-benchmark.yml` therefore performs the privileged proof here, using the existing `checkout-remote-libs` composite action and the narrowly scoped, read-only `K8S_LIBS_DEPLOY_KEY` repository secret. It never accepts a personal access token, never embeds a credential in a Git URL, and never writes the key to a report or artifact.

For each native architecture, the workflow:

1. resolves and checks out the exact `remote/libs` and Scintilla monorepo gitlinks;
2. overlays an explicitly selected public runner commit at the canonical nested path;
3. builds the compatibility and lean images from the cluster root context;
4. verifies the loaded OCI architecture;
5. starts fresh containers in alternating order and measures `docker run` through the first HTTP 200 from `/healthz`;
6. publishes JSON and Markdown evidence with image sizes, raw samples, median, and p95.

The pull request introducing this workflow benchmarks runner commit `7c636a3f5da466816005ae0d53d6634641e820e9`. After merge, `workflow_dispatch` can benchmark any public branch, tag, or immutable commit without changing the cluster gitlinks. The lean runner remains a draft until both architecture jobs complete successfully and the resulting evidence is reviewed.

These measurements isolate container image startup on native x86_64 and arm64 GitHub-hosted machines. They are not AWS Lambda `Init Duration`; that separate measurement requires published Lambda versions and an AWS benchmark account, role, and log-retention policy.
