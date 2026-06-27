# syntax=docker/dockerfile:1
# Tooling image for submodule pinning and branch coordination workflows.
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash git ca-certificates
WORKDIR /workspace/fiducia-monorepo
COPY .gitmodules readme.md ./
COPY docs docs
COPY scripts scripts
CMD ["bash", "-lc", "scripts/pin-submodules.sh --help && scripts/checkout-feature-branch.sh --help"]
