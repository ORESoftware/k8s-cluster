FROM docker.io/library/rust:1.90-alpine AS build
WORKDIR /build
COPY runtime-images/common/rust-handler.rs ./rust-handler.rs
RUN mkdir -p /out \
  && rustc -C opt-level=3 -o /out/dd-pool-rust-handler rust-handler.rs

FROM docker.io/library/alpine:3.22
RUN apk add --no-cache python3 ca-certificates \
  && addgroup -S pool \
  && adduser -S -G pool -u 10001 pool
WORKDIR /opt/dd-container-pool
COPY runtime-images/common/worker.py ./worker.py
COPY --from=build /out/dd-pool-rust-handler /usr/local/bin/dd-pool-rust-handler
ENV PORT=8080
ENV DD_POOL_RUNTIME=rust
ENV DD_POOL_HANDLER=/usr/local/bin/dd-pool-rust-handler
USER 10001:10001
ENTRYPOINT ["python3", "/opt/dd-container-pool/worker.py"]
