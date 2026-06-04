FROM docker.io/library/alpine:edge AS assets
WORKDIR /opt/dd-container-pool
COPY runtime-images/common/worker.py ./worker.py
COPY runtime-images/common/nodejs-handler.mjs ./handlers/nodejs-handler.mjs

FROM docker.io/library/alpine:edge
RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  nodejs-current \
  python3 \
  && addgroup -S pool \
  && adduser -S -G pool -u 10001 pool
WORKDIR /opt/dd-container-pool
COPY --from=assets /opt/dd-container-pool /opt/dd-container-pool
ENV PORT=8080
ENV DD_POOL_RUNTIME=nodejs
ENV DD_POOL_HANDLER="node /opt/dd-container-pool/handlers/nodejs-handler.mjs"
USER 10001:10001
ENTRYPOINT ["python3", "/opt/dd-container-pool/worker.py"]
