FROM rust:1.90-bookworm AS build

WORKDIR /app

# Build context must be the repo root:
#   docker build -f remote/deployments/dd-data-viz-rs/Dockerfile -t dd-data-viz-rs:dev .
COPY remote/deployments/dd-data-viz-rs/Cargo.toml /app/remote/deployments/dd-data-viz-rs/Cargo.toml
COPY remote/deployments/dd-data-viz-rs/Cargo.lock /app/remote/deployments/dd-data-viz-rs/Cargo.lock
COPY remote/deployments/dd-data-viz-rs/src /app/remote/deployments/dd-data-viz-rs/src

WORKDIR /app/remote/deployments/dd-data-viz-rs
RUN cargo build --release --locked

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean

COPY --from=build /app/remote/deployments/dd-data-viz-rs/target/release/dd-data-viz-rs /usr/local/bin/dd-data-viz-rs

ENV HOST=0.0.0.0
ENV PORT=8126

EXPOSE 8126

CMD ["/usr/local/bin/dd-data-viz-rs"]
