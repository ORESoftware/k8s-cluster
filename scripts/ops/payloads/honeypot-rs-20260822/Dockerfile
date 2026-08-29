FROM rust:1.98.0-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
RUN cargo build --release --locked

FROM scratch
COPY --from=builder /src/target/release/honeypot-rs /honeypot-rs
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/honeypot-rs"]
