FROM rust:1.90-bookworm AS build
WORKDIR /repo
COPY remote/deployments/formal-methods-server-rs ./remote/deployments/formal-methods-server-rs
COPY remote/libs/runtime-config-client-rs ./remote/libs/runtime-config-client-rs
WORKDIR /repo/remote/deployments/formal-methods-server-rs
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates git openssh-client z3 \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /repo/remote/deployments/formal-methods-server-rs/target/release/dd-formal-methods-server /usr/local/bin/dd-formal-methods-server
ENTRYPOINT ["/usr/local/bin/dd-formal-methods-server"]
