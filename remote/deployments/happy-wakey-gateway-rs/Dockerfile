FROM rust:1.91.1-bookworm AS build
WORKDIR /source
COPY . .
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home /nonexistent --shell /usr/sbin/nologin happy-wakey
COPY --from=build /source/target/release/happy-wakey-gateway-rs /usr/local/bin/happy-wakey-gateway-rs
USER 10001:10001
EXPOSE 8128
ENTRYPOINT ["/usr/local/bin/happy-wakey-gateway-rs"]
