FROM rust:1.87-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

FROM alpine:3.20
RUN apk add --no-cache ca-certificates
COPY --from=build /app/target/release/idle-reaper /usr/local/bin/idle-reaper
ENTRYPOINT ["/usr/local/bin/idle-reaper"]
