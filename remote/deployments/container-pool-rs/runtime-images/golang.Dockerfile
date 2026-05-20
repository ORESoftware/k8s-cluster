FROM docker.io/library/golang:1.25-alpine AS build
WORKDIR /build
COPY runtime-images/common/golang-handler.go ./golang-handler.go
RUN mkdir -p /out \
  && CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /out/dd-pool-golang-handler golang-handler.go

FROM docker.io/library/alpine:3.22
RUN apk add --no-cache python3 ca-certificates \
  && addgroup -S pool \
  && adduser -S -G pool -u 10001 pool
WORKDIR /opt/dd-container-pool
COPY runtime-images/common/worker.py ./worker.py
COPY --from=build /out/dd-pool-golang-handler /usr/local/bin/dd-pool-golang-handler
ENV PORT=8080
ENV DD_POOL_RUNTIME=golang
ENV DD_POOL_HANDLER=/usr/local/bin/dd-pool-golang-handler
USER 10001:10001
ENTRYPOINT ["python3", "/opt/dd-container-pool/worker.py"]
