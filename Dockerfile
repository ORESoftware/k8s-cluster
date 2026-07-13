# syntax=docker/dockerfile:1
# Tooling image for submodule pinning and branch coordination workflows.
FROM debian:bookworm-slim@sha256:60eac759739651111db372c07be67863818726f754804b8707c90979bda511df
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash git ca-certificates
WORKDIR /workspace/fiducia-monorepo
COPY .gitmodules readme.md ./
COPY docs docs
COPY scripts scripts
CMD ["bash", "-lc", "scripts/pin-submodules.sh --help && scripts/checkout-feature-branch.sh --help"]
