# Multi-stage build → small runtime image, published by CI as
# ghcr.io/shared-auth/shared-auth-nats-bridge:<tag>. Never compiled in-pod.
FROM rust:1.96-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && echo '' > src/lib.rs \
    && cargo build --release --locked || true
COPY . .
RUN touch src/main.rs src/lib.rs && cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home appuser
COPY --from=build /src/target/release/shared-auth-nats-bridge /usr/local/bin/shared-auth-nats-bridge
USER 10001
EXPOSE 8121
ENTRYPOINT ["/usr/local/bin/shared-auth-nats-bridge"]
