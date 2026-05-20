FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine AS assets
WORKDIR /opt/dd-container-pool
COPY runtime-images/common/worker.py ./worker.py
COPY runtime-images/common/erlang-handler.escript ./handlers/erlang-handler.escript

FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine
RUN apk add --no-cache python3 \
  && addgroup -S pool \
  && adduser -S -G pool -u 10001 pool
WORKDIR /opt/dd-container-pool
COPY --from=assets /opt/dd-container-pool /opt/dd-container-pool
ENV PORT=8080
ENV DD_POOL_RUNTIME=gleamlang
ENV DD_POOL_HANDLER="escript /opt/dd-container-pool/handlers/erlang-handler.escript"
USER 10001:10001
ENTRYPOINT ["python3", "/opt/dd-container-pool/worker.py"]
