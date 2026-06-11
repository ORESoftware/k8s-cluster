# syntax=docker/dockerfile:1
FROM rust:1.95-bookworm AS build

WORKDIR /app

# Build context must be the repo root:
#   docker build -f remote/deployments/dd-benefactor-marketing-rs/Dockerfile -t dd-benefactor-marketing-rs:dev .
COPY remote/libs/pg-defs/generated/rust/sea-orm /app/remote/libs/pg-defs/generated/rust/sea-orm
COPY remote/deployments/dd-benefactor-marketing-rs/Cargo.toml /app/remote/deployments/dd-benefactor-marketing-rs/Cargo.toml
COPY remote/deployments/dd-benefactor-marketing-rs/Cargo.lock /app/remote/deployments/dd-benefactor-marketing-rs/Cargo.lock
COPY remote/deployments/dd-benefactor-marketing-rs/src /app/remote/deployments/dd-benefactor-marketing-rs/src
COPY remote/deployments/dd-benefactor-marketing-rs/generated /app/remote/deployments/dd-benefactor-marketing-rs/generated

WORKDIR /app/remote/deployments/dd-benefactor-marketing-rs
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/remote/deployments/dd-benefactor-marketing-rs/target,id=dd-benefactor-marketing-rs-target,sharing=locked \
    cargo build --release --locked \
 && cp target/release/dd-benefactor-marketing-rs /usr/local/bin/dd-benefactor-marketing-rs

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean \
  && rm -rf /var/lib/apt/lists/*

COPY --from=build /usr/local/bin/dd-benefactor-marketing-rs /usr/local/bin/dd-benefactor-marketing-rs

ENV BENEFACTOR_MARKETING_HOST=0.0.0.0
ENV BENEFACTOR_MARKETING_PORT=8134

EXPOSE 8134

USER 65532:65532

CMD ["/usr/local/bin/dd-benefactor-marketing-rs"]
