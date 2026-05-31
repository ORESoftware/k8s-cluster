FROM rust:1.90-bookworm AS build

WORKDIR /app

# The shared runtime-config client is a sibling path dep
# (`../../libs/runtime-config-client-rs`), so the build context for
# `docker build` must be the repo root, e.g.:
#   docker build -f remote/deployments/rust-vapi-phone-rs/Dockerfile -t dd-rust-vapi-phone:dev .
COPY remote/libs/runtime-config-client-rs /app/remote/libs/runtime-config-client-rs
COPY remote/deployments/rust-vapi-phone-rs/Cargo.toml /app/remote/deployments/rust-vapi-phone-rs/Cargo.toml
COPY remote/deployments/rust-vapi-phone-rs/Cargo.lock /app/remote/deployments/rust-vapi-phone-rs/Cargo.lock
COPY remote/deployments/rust-vapi-phone-rs/src /app/remote/deployments/rust-vapi-phone-rs/src
COPY remote/deployments/rust-vapi-phone-rs/generated /app/remote/deployments/rust-vapi-phone-rs/generated

WORKDIR /app/remote/deployments/rust-vapi-phone-rs
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean
COPY --from=build /app/remote/deployments/rust-vapi-phone-rs/target/release/dd-rust-vapi-phone /usr/local/bin/dd-rust-vapi-phone

ENV HOST=0.0.0.0
ENV PORT=8113

EXPOSE 8113

CMD ["/usr/local/bin/dd-rust-vapi-phone"]
