FROM rust:1.90-bookworm AS build
WORKDIR /app
COPY remote/deployments/usacc-rest-api-backend-rs ./remote/deployments/usacc-rest-api-backend-rs
COPY remote/libs/pg-defs/generated/rust ./remote/libs/pg-defs/generated/rust
COPY remote/submodules/discrete-event-system.rs ./remote/submodules/discrete-event-system.rs
WORKDIR /app/remote/deployments/usacc-rest-api-backend-rs
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates
COPY --from=build /app/remote/deployments/usacc-rest-api-backend-rs/target/release/usacc-rest-api-backend-rs /usr/local/bin/usacc-rest-api-backend-rs
ENTRYPOINT ["/usr/local/bin/usacc-rest-api-backend-rs"]
