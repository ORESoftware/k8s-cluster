FROM docker.io/library/python:3.13-alpine AS assets
WORKDIR /opt/dd-container-pool
COPY runtime-images/common/worker.py ./worker.py
COPY runtime-images/common/python3_handler.py ./handlers/python3_handler.py

FROM docker.io/library/python:3.13-alpine
RUN addgroup -S pool && adduser -S -G pool -u 10001 pool
WORKDIR /opt/dd-container-pool
COPY --from=assets /opt/dd-container-pool /opt/dd-container-pool
ENV PORT=8080
ENV DD_POOL_RUNTIME=python3
ENV DD_POOL_HANDLER="python3 /opt/dd-container-pool/handlers/python3_handler.py"
USER 10001:10001
ENTRYPOINT ["python3", "/opt/dd-container-pool/worker.py"]
