FROM rust:1.86-bookworm AS build

WORKDIR /app
COPY Cargo.toml ./Cargo.toml
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean

COPY --from=build /app/target/release/dd-remote-rest-api /usr/local/bin/dd-remote-rest-api

ENV HOST=0.0.0.0
ENV PORT=8082

EXPOSE 8082

CMD ["/usr/local/bin/dd-remote-rest-api"]
