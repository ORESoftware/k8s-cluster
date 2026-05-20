FROM docker.io/library/dart:stable AS build
WORKDIR /build
COPY runtime-images/common/dart-handler.dart ./dart-handler.dart
RUN dart compile exe dart-handler.dart -o /out/dd-pool-dart-handler

FROM docker.io/library/debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates python3 \
  && rm -rf /var/lib/apt/lists/* \
  && groupadd --system pool \
  && useradd --system --gid pool --uid 10001 --home-dir /nonexistent pool
WORKDIR /opt/dd-container-pool
COPY runtime-images/common/worker.py ./worker.py
COPY --from=build /out/dd-pool-dart-handler /usr/local/bin/dd-pool-dart-handler
ENV PORT=8080
ENV DD_POOL_RUNTIME=dart
ENV DD_POOL_HANDLER=/usr/local/bin/dd-pool-dart-handler
USER 10001:10001
ENTRYPOINT ["python3", "/opt/dd-container-pool/worker.py"]
