# Rust Dockerfile build caching

All `*-rs` deployment Dockerfiles use **BuildKit cargo cache mounts** so that
crate downloads and compiled artifacts persist across image builds. This is the
canonical pattern — prefer it over the older `echo "fn main(){}"` dependency
pre-build trick (which is fragile with path deps / multiple bins and is now
removed everywhere).

## Requirements

- BuildKit (the in-cluster `build-server-rs` shells out to `nerdctl build`,
  which uses buildkitd; `docker build` uses BuildKit by default). The mounts are
  a no-op cost on a cold builder, so builds still succeed without a warm cache.
- The first line of the Dockerfile **must** be `# syntax=docker/dockerfile:1`.
  A syntax directive placed after any comment or blank line is silently ignored.
- Cache persistence requires a persistent buildkitd cache backend. With an
  ephemeral builder the mounts are correct but cold every build.

## The pattern

```dockerfile
# syntax=docker/dockerfile:1
FROM rust:1.90-bookworm AS build
ARG TARGETARCH
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/target,id=<service>-target-${TARGETARCH},sharing=locked \
    cargo build --release \
 && cp target/release/<binary> /usr/local/bin/<binary>

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/<binary> /usr/local/bin/<binary>
ENTRYPOINT ["/usr/local/bin/<binary>"]
```

### Why each piece matters

- **`registry` + `git` mounts** are shared across every service (`id=cargo-registry`,
  `id=cargo-git`) so a crate downloaded for one service is reused by all. The
  cargo registry/git db holds *downloaded sources + the index* — architecture
  independent, so sharing across services and target platforms is safe.
- **`target` mount** is per-service AND per-arch (`id=<service>-target-${TARGETARCH}`)
  so two services never thrash each other's incremental dir, and — critically —
  a multi-arch build (`--platform linux/amd64,linux/arm64`) never mixes
  arch-specific object files in one cache. `${TARGETARCH}` is auto-populated by
  BuildKit (declare `ARG TARGETARCH` in the build stage); on a single-arch build
  it resolves to the host arch, so the id is stable.
- **`sharing=locked`** serializes concurrent builders on the same cache id. This
  is required for correctness here: cargo's cross-process lock lives at
  `$CARGO_HOME/.package-cache`, which is *not* one of the mounted paths, so
  concurrent builds sharing the registry mount would otherwise have no lock
  coordinating their writes. The trade-off is that concurrent Rust image builds
  serialize on the shared registry/git caches; we prefer that to corruption.
- **`cp target/release/<binary> /usr/local/bin/<binary>`** is mandatory: a
  cache-mounted `target/` is *not* part of the image filesystem, so the binary
  must be copied to a real path inside the same `RUN`. The runtime stage then
  `COPY --from=build /usr/local/bin/<binary>` — never from `target/release`.
  The `<binary>` name must match what cargo emits: the crate's `[[bin]]` name,
  or the `[package] name` if there is no explicit `[[bin]]`.

### Operational note

Cache mounts grow unbounded on the buildkitd host. Cap them with a buildkitd GC
policy (e.g. `keepBytes` / `keepDuration` in `buildkitd.toml`, or
`docker builder prune --filter type=exec.cachemount`) so the per-service
`target/` caches don't fill the build node's disk.

## Notes

- `target=` paths must be absolute. For repo-root build contexts the crate's
  `target/` is at `<crate workdir>/target` (the crates are not a cargo
  workspace, so each has its own `target/`).
- The official `rust` images (bookworm and alpine) set `CARGO_HOME=/usr/local/cargo`,
  so the registry/git mount paths above are correct for both.
- Native-dependency builds (`dd-ocr-rs`, `dd-document-rs`, `dd-git-rs`) keep
  their extra build stages and `strip`; the cache mounts attach to the Rust
  build `RUN` exactly as above.

Validate any change with `docker build --check -f <Dockerfile> <dir>` — it lints
the Dockerfile (including the `--mount` flags) without running a full build.
