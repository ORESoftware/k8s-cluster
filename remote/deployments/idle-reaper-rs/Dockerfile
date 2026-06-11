# syntax=docker/dockerfile:1
FROM rust:1.87-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/target,id=idle-reaper-rs-target,sharing=locked \
    cargo build --release \
 && cp target/release/idle-reaper /usr/local/bin/idle-reaper

FROM alpine:3.20
RUN apk add --no-cache ca-certificates
COPY --from=build /usr/local/bin/idle-reaper /usr/local/bin/idle-reaper
ENTRYPOINT ["/usr/local/bin/idle-reaper"]
