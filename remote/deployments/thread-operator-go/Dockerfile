FROM docker.io/library/golang:1.23-bookworm AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY api ./api
COPY cmd ./cmd
COPY internal ./internal
RUN CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /out/dd-thread-operator ./cmd/operator

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /out/dd-thread-operator /usr/local/bin/dd-thread-operator
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/dd-thread-operator"]
