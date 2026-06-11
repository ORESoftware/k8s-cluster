# syntax=docker/dockerfile:1
FROM rust:1.86-bookworm AS build

WORKDIR /app
COPY Cargo.toml ./Cargo.toml
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/target,id=rest-api-rs-target,sharing=locked \
    cargo build --release \
 && cp target/release/dd-remote-rest-api /usr/local/bin/dd-remote-rest-api

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean

COPY --from=build /usr/local/bin/dd-remote-rest-api /usr/local/bin/dd-remote-rest-api

ENV HOST=0.0.0.0
ENV PORT=8082

EXPOSE 8082

CMD ["/usr/local/bin/dd-remote-rest-api"]
