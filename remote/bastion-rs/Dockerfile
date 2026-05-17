FROM rust:1.90-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl util-linux \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/dd-bastion /usr/local/bin/dd-bastion
ENTRYPOINT ["/usr/local/bin/dd-bastion"]
