FROM rust:1.88-bookworm AS builder
RUN apt-get update \
    && apt-get install --yes --no-install-recommends libssl-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
# Dependency layer: compile the full locked graph against stub sources so a
# source-only change does not rebuild every crate at codegen-units=1. The
# lockfile is copied without a glob so its absence fails here, not later
# inside cargo with a confusing --locked error.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && touch src/lib.rs \
    && cargo build --locked --release \
    && rm -rf src
COPY src ./src
RUN touch src/main.rs src/lib.rs && cargo build --locked --release

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --create-home app \
    && apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/push-notification-server /usr/local/bin/push-notification-server
USER 10001
EXPOSE 8121
ENTRYPOINT ["/usr/local/bin/push-notification-server"]
