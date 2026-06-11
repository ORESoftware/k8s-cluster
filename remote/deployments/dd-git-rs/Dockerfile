# syntax=docker/dockerfile:1
# dd-git-rs ships as a built image rather than the cargo-run-from-hostPath
# pattern the sibling dd-*-rs services use, so the running container can be
# non-root with a read-only root filesystem and ship the four VCS CLIs baked in
# (no apt-as-root at startup).
#
# IMPORTANT: build from the cluster repo's `remote/` dir so the Cargo
# path-dependency under libs/ is in the build context:
#
#   docker build -f deployments/dd-git-rs/Dockerfile -t dd-git-rs .
#
# (The repo-root .dockerignore keeps the context to just this crate + pg-defs.)


# ---------------------------------------------------------------------------
# 1) Build the Rust service. The repo layout is replicated under /src so the
#    crate's ../../libs/pg-defs path dependency resolves.
# ---------------------------------------------------------------------------
FROM rust:1.91-bookworm AS build
WORKDIR /src
# Cargo path-dependency: the generated pg-defs constants crate.
COPY libs/pg-defs/generated/rust ./libs/pg-defs/generated/rust
COPY deployments/dd-git-rs ./deployments/dd-git-rs
WORKDIR /src/deployments/dd-git-rs
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/src/deployments/dd-git-rs/target,id=dd-git-rs-target,sharing=locked \
    cargo build --release --locked \
 && strip target/release/dd-git-rs \
 && cp target/release/dd-git-rs /usr/local/bin/dd-git-rs

# ---------------------------------------------------------------------------
# 2) Runtime image: the four VCS CLIs the server drives, TLS roots, and an SSH
#    client for ssh:// remotes. No build toolchain, no root at runtime.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
       git mercurial subversion fossil ca-certificates openssh-client \
  && rm -rf /var/lib/apt/lists/*
# Non-root runtime user. The storage root is owned by it so the read-only-rootfs
# deployment can still write mirrors to the mounted volume.
RUN groupadd -g 1000 ddgit \
  && useradd -u 1000 -g 1000 -m -d /home/ddgit -s /usr/sbin/nologin ddgit \
  && mkdir -p /var/lib/dd-git-rs/repos \
  && chown -R 1000:1000 /var/lib/dd-git-rs
COPY --from=build /usr/local/bin/dd-git-rs \
     /usr/local/bin/dd-git-rs
ENV GIT_RS_STORAGE_ROOT=/var/lib/dd-git-rs/repos \
    HOME=/tmp
USER 1000:1000
EXPOSE 8137
ENTRYPOINT ["/usr/local/bin/dd-git-rs"]
