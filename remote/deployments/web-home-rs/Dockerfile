# syntax=docker/dockerfile:1.7

FROM rust:1.90-bookworm AS build
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY --from=build /app/target/release/dd-remote-web-home /usr/local/bin/dd-remote-web-home

ENV HOST=0.0.0.0 \
    PORT=8080

EXPOSE 8080
USER 10001:10001
CMD ["/usr/local/bin/dd-remote-web-home"]
