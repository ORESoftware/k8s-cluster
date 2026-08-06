# syntax=docker/dockerfile:1
# Tooling image for submodule pinning and branch coordination workflows.
FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241
LABEL org.fiducia.runtime-profile="tool-runner-nonroot"
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash git ca-certificates \
    && apt-get clean \
    && install -d -o 65532 -g 65532 /workspace/fiducia-monorepo /tmp
WORKDIR /workspace/fiducia-monorepo
COPY --chown=65532:65532 .gitmodules readme.md ./
COPY --chown=65532:65532 docs docs
COPY --chown=65532:65532 scripts scripts
ENV HOME=/tmp
USER 65532:65532
CMD ["bash", "-lc", "scripts/pin-submodules.sh --help && scripts/checkout-feature-branch.sh --help"]
