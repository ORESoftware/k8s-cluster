# Multi-stage build → small runtime image. Built and published by CI as
# ghcr.io/oresoftware/shared-auth-server:<tag>, NOT compiled in-pod (see the
# k8s-cluster scaling docs on why in-pod cargo builds pin pods to a node).
FROM rust:1.96-bookworm AS build
WORKDIR /src
# Cache deps.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && echo '' > src/lib.rs \
    && cargo build --release --locked || true
COPY . .
RUN touch src/main.rs src/lib.rs && cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home appuser
COPY --from=build /src/target/release/shared-auth-server /usr/local/bin/shared-auth-server
USER 10001
EXPOSE 8120
ENTRYPOINT ["/usr/local/bin/shared-auth-server"]
CMD ["serve"]
