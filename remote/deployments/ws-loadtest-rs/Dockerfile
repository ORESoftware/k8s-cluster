# syntax=docker/dockerfile:1
FROM docker.io/library/rust:1.86-alpine AS build
ARG TARGETARCH
WORKDIR /app

RUN apk add --no-cache musl-dev
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/target,id=ws-loadtest-rs-target-${TARGETARCH},sharing=locked \
    cargo build --release \
 && cp target/release/ws-loadtest-rs /usr/local/bin/ws-loadtest-rs

FROM docker.io/library/alpine:3.20
WORKDIR /app

RUN apk add --no-cache ca-certificates
COPY --from=build /usr/local/bin/ws-loadtest-rs /app/ws-loadtest-rs

CMD ["/app/ws-loadtest-rs"]
