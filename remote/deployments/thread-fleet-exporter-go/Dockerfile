FROM docker.io/library/golang:1.23-bookworm AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY cmd ./cmd
RUN CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /out/dd-thread-fleet-exporter ./cmd/exporter

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /out/dd-thread-fleet-exporter /usr/local/bin/dd-thread-fleet-exporter
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/dd-thread-fleet-exporter"]
