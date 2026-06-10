FROM rust:1.90-bookworm AS build

WORKDIR /app

# Build context must be the repo root:
#   docker build -f remote/deployments/patent-filing-rs/Dockerfile -t dd-patent-filing-rs:dev .
COPY remote/deployments/patent-filing-rs/Cargo.toml /app/remote/deployments/patent-filing-rs/Cargo.toml
COPY remote/deployments/patent-filing-rs/Cargo.lock /app/remote/deployments/patent-filing-rs/Cargo.lock
COPY remote/deployments/patent-filing-rs/src /app/remote/deployments/patent-filing-rs/src
COPY remote/deployments/patent-filing-rs/generated /app/remote/deployments/patent-filing-rs/generated
COPY remote/libs /app/remote/libs

WORKDIR /app/remote/deployments/patent-filing-rs
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*

COPY --from=build /app/remote/deployments/patent-filing-rs/target/release/dd-patent-filing-rs /usr/local/bin/dd-patent-filing-rs

ENV HOST=0.0.0.0
ENV PORT=8116

EXPOSE 8116

CMD ["/usr/local/bin/dd-patent-filing-rs"]
