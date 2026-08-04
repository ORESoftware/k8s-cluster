# Standalone source and publication contract

The canonical source for this repository is:

```text
repository: ORESoftware/k8s-cluster
path: remote/deployments/gha-clone-server-rs
standalone target: gha-indie-worker/gha-clone-server.rs
```

The standalone repository is a reviewed extraction, not an independently edited fork. A publication must copy the complete source directory at one immutable 40-hex commit, preserve file modes and `Cargo.lock`, record the source commit in the publication PR or commit message, and compare the target tree against the extracted source before updating `main`.

The extraction includes its own `.github/workflows/ci.yml` and `.github/workflows/gha-clone-server-meta.yml`. Those workflows are intentionally stored inside the source directory so they become root-level GitHub Actions workflows in the standalone repository while remaining inert inside the monorepo.

Publication must fail closed when:

- the source revision is not a full immutable commit;
- any source file, hidden file, executable bit, or lockfile is omitted;
- the target contains an unreviewed manual-only file;
- the standalone meta test reaches outside the repository;
- the target tree differs from the reviewed extraction;
- the native Rust, real-process, image, or secret-scan matrix is not green.

The standalone repository may receive ordinary pull requests after extraction, but any source synchronization must be a semantic PR that reconciles those changes with the canonical monorepo implementation. It must never force-push over independently reviewed history or choose an entire conflict side without preserving both intended meanings.
