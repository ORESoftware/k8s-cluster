FROM rust:1.90-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY generated ./generated
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates
COPY --from=build /app/target/release/dd-escrow-rs /usr/local/bin/dd-escrow-rs
ENTRYPOINT ["/usr/local/bin/dd-escrow-rs"]
