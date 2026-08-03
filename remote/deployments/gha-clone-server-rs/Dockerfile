# syntax=docker/dockerfile:1.7
FROM docker.io/library/rust:1.90-bookworm AS builder
WORKDIR /src
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

FROM docker.io/library/debian:bookworm-slim
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates \
    && apt-get clean
RUN groupadd --gid 10001 app \
    && useradd --uid 10001 --gid 10001 --home-dir /nonexistent --shell /usr/sbin/nologin app
COPY --from=builder /src/target/release/gha-clone-server /usr/local/bin/gha-clone-server
USER 10001:10001
EXPOSE 8117
ENTRYPOINT ["/usr/local/bin/gha-clone-server"]
