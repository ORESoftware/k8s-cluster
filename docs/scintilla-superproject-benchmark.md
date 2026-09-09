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

The App selector and trusted bootstrap are tracked in PR #1460. That bootstrap
must validate the same App across the authoritative allowlist before writing the
repository Actions secrets. No personal token is committed, accepted as a
workflow input, embedded in a Git URL, or written to evidence.

For each native architecture, the workflow:

1. resolves the immutable shared-library and Scintilla monorepo gitlinks from
   the checked-out superproject;
2. mints a one-repository `contents:read` token for each checked-out private
   repository;
3. overlays the exact runner candidate at its canonical nested path;
4. verifies the shared-library SHA, retained monorepo gitlink, and candidate
   runner SHA independently;
5. verifies the immutable
   `ORESoftware/typespec-json-schema-validator` CI pin and both independently
   authored runtime-wire authorities;
6. builds the generated compatibility image and hand-authored lean image from
   the cluster-root context;
7. verifies the loaded OCI architecture;
8. starts fresh containers in alternating order and measures `docker run`
   through the first HTTP 200 from `/healthz`; and
9. publishes JSON, Markdown, provenance, image-size, raw-sample, median, and p95
   evidence.

The compatibility and lean files deliberately have different ownership and
names:

| Architecture | Generated compatibility Dockerfile | Hand-authored lean Dockerfile |
| --- | --- | --- |
| amd64 | `Dockerfile.x86-64.dkf` | `runtime-images/control-plane-lean-amd64.Dockerfile` |
| arm64 | `Dockerfile.arm64.dkf` | `runtime-images/control-plane-lean-arm64.Dockerfile` |

The current pull request benchmarks runner commit
`95014de1c5fe799a8c313e20eff8c40dba88d32b`. After merge,
`workflow_dispatch` can benchmark another branch, tag, or immutable commit
without changing the cluster gitlinks.

The runner remains draft until both native architecture jobs complete
successfully, the TJSV receipt for the candidate head is passed and admissible,
and the resulting benchmark artifacts are reviewed.

These measurements isolate container-image startup on native x86_64 and arm64
GitHub-hosted machines. They are not AWS Lambda `Init Duration`; that separate
measurement requires published Lambda versions and an AWS benchmark account,
role, and log-retention policy.
