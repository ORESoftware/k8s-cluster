FROM docker.io/library/rust:1.82-alpine AS build
WORKDIR /app

RUN apk add --no-cache musl-dev
COPY . .
RUN cargo build --release

FROM docker.io/library/alpine:3.20
WORKDIR /app

RUN apk add --no-cache ca-certificates
COPY --from=build /app/target/release/ws-loadtest-rs /app/ws-loadtest-rs

CMD ["/app/ws-loadtest-rs"]
