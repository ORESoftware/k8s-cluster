FROM rust:1.90-bookworm AS build

WORKDIR /app

# Build context must be the repo root:
#   docker build -f remote/deployments/apostille-services-server-rs/Dockerfile -t dd-apostille-services-server:dev .
COPY remote/deployments/apostille-services-server-rs/Cargo.toml /app/remote/deployments/apostille-services-server-rs/Cargo.toml
COPY remote/deployments/apostille-services-server-rs/Cargo.lock /app/remote/deployments/apostille-services-server-rs/Cargo.lock
COPY remote/deployments/apostille-services-server-rs/src /app/remote/deployments/apostille-services-server-rs/src
COPY remote/deployments/apostille-services-server-rs/generated /app/remote/deployments/apostille-services-server-rs/generated

WORKDIR /app/remote/deployments/apostille-services-server-rs
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*

COPY --from=build /app/remote/deployments/apostille-services-server-rs/target/release/dd-apostille-services-server /usr/local/bin/dd-apostille-services-server

ENV HOST=0.0.0.0
ENV PORT=8122

EXPOSE 8122

CMD ["/usr/local/bin/dd-apostille-services-server"]
