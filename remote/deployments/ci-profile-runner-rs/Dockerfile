# syntax=docker/dockerfile:1.7
FROM docker.io/library/rust:1.90-bookworm AS builder

WORKDIR /src
COPY remote/deployments/ci-profile-runner-rs/Cargo.toml remote/deployments/ci-profile-runner-rs/Cargo.lock ./remote/deployments/ci-profile-runner-rs/
COPY remote/deployments/ci-profile-runner-rs/src ./remote/deployments/ci-profile-runner-rs/src
COPY remote/libs/telemetry-rs ./remote/libs/telemetry-rs
RUN cargo build --locked --release --manifest-path remote/deployments/ci-profile-runner-rs/Cargo.toml

FROM docker.io/library/debian:bookworm-slim
ARG OCI_CREATED
ARG OCI_REVISION
ARG OCI_SOURCE
LABEL org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.source="${OCI_SOURCE}" \
      org.opencontainers.image.title="dd-ci-profile-runner"
RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates git \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/remote/deployments/ci-profile-runner-rs/target/release/dd-ci-profile-runner /usr/local/bin/dd-ci-profile-runner
ENTRYPOINT ["/usr/local/bin/dd-ci-profile-runner"]
