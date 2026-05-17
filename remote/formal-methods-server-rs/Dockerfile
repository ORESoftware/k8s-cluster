FROM rust:1.90-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates git openssh-client z3 \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/dd-formal-methods-server /usr/local/bin/dd-formal-methods-server
ENTRYPOINT ["/usr/local/bin/dd-formal-methods-server"]
