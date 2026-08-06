FROM docker.io/library/rust:1.90-bookworm AS build
WORKDIR /workspace
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /workspace/target/release/dd-durable-worker-server /usr/local/bin/dd-durable-worker-server
USER nonroot:nonroot
EXPOSE 8152
ENTRYPOINT ["/usr/local/bin/dd-durable-worker-server"]
