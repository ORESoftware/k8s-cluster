# Scintilla full-superproject multi-architecture benchmark

This repository owns the authoritative build context required by
`scintilla-run/gleam-lambda-runner`. Repository-scoped `GITHUB_TOKEN` values in
the runner repository cannot prove a build that traverses sibling private
repositories, so native integration evidence is produced here.

`.github/workflows/scintilla-superproject-benchmark.yml` uses one reviewed,
read-only GitHub App credential pair and mints separate, repository-restricted
installation tokens for:

- the exact `ORESoftware/k8s-libs-and-shared-defs` gitlink; and
- the exact `scintilla-run/gleam-lambda-runner` candidate revision.

The App selector and trusted bootstrap were merged through PR #1460. The
bootstrap must validate the same App across the authoritative allowlist before
writing the repository Actions secrets. No personal token is committed,
accepted as a workflow input, embedded in a Git URL, or written to evidence.

For each native architecture, the workflow:

1. resolves the immutable shared-library and Scintilla monorepo gitlinks from
   the checked-out superproject;
2. mints a one-repository `contents:read` token for each checked-out private
   repository;
3. overlays the exact runner candidate at its canonical nested path;
4. verifies the shared-library SHA, retained monorepo gitlink, and candidate
   runner SHA independently;
5. executes the Node.js, Python, Ruby, and Bash child adapters in check mode,
   covering both valid function bodies and syntax failures;
6. records the observed requests, success responses, and failure responses as
   an instance corpus;
7. runs the immutable
   `ORESoftware/typespec-json-schema-validator` action over the independently
   authored TypeSpec and JSON Schema authorities plus that observed corpus;
8. requires a passed parity receipt, zero findings, and an admissible,
   non-authoritative Contract IR before any image build;
9. builds the generated compatibility image and hand-authored lean image from
   the cluster-root context;
10. verifies the loaded OCI architecture;
11. starts fresh containers in alternating order and measures `docker run`
    through the first HTTP 200 from `/healthz`; and
12. publishes JSON, Markdown, provenance, runtime-contract, image-size,
    raw-sample, median, and p95 evidence.

The compatibility and lean files deliberately have different ownership and
names:

| Architecture | Generated compatibility Dockerfile | Hand-authored lean Dockerfile |
| --- | --- | --- |
| amd64 | `Dockerfile.x86-64.dkf` | `runtime-images/control-plane-lean-amd64.Dockerfile` |
| arm64 | `Dockerfile.arm64.dkf` | `runtime-images/control-plane-lean-arm64.Dockerfile` |

The current pull request benchmarks runner commit
`25689f71870a4b975ec519d384a30fd2a1409626`. After merge,
`workflow_dispatch` can benchmark another branch, tag, or immutable commit
without changing the cluster gitlinks.

## Current operational blocker

The first trusted-main bootstrap run after PR #1460 passed the selector
self-test and exact 32-repository allowlist validation, then failed before
secret hydration because AWS rejected `sts:AssumeRoleWithWebIdentity` for the
configured OIDC role. The benchmark must not fall back to a personal access
token. The AWS role trust relationship or selected repository secret must be
corrected, the trusted bootstrap rerun, and both native jobs rerun on the exact
candidate above.

The runner remains draft until both native architecture jobs complete
successfully, the TJSV receipt for the candidate head is passed and admissible,
and the resulting benchmark artifacts are reviewed.

These measurements isolate container-image startup on native x86_64 and arm64
GitHub-hosted machines. They are not AWS Lambda `Init Duration`; that separate
measurement requires published Lambda versions and an AWS benchmark account,
role, and log-retention policy.
