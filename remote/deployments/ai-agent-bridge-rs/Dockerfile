# syntax=docker/dockerfile:1
FROM rust:1.87-alpine AS build
ARG TARGETARCH
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/target,id=ai-agent-bridge-rs-target-${TARGETARCH},sharing=locked \
    cargo build --release \
 && cp target/release/ai-agent-bridge /usr/local/bin/ai-agent-bridge

FROM alpine:3.20
RUN apk add --no-cache ca-certificates
COPY --from=build /usr/local/bin/ai-agent-bridge /usr/local/bin/ai-agent-bridge
ENTRYPOINT ["/usr/local/bin/ai-agent-bridge"]
