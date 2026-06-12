FROM docker.io/library/python:3.12-slim@sha256:090ba77e2958f6af52a5341f788b50b032dd4ca28377d2893dcf1ecbdfdfe203

RUN groupadd --system pipeline && useradd --system --gid pipeline --uid 10001 pipeline
WORKDIR /opt/dd-next-1/remote/deployments/ai-ml-pipeline

# OpenTelemetry SDK + OTLP/HTTP exporter for explicit request tracing (src/otel.py).
# The service degrades gracefully if these are absent, but we ship them in the image.
RUN pip install --no-cache-dir \
    "opentelemetry-sdk==1.29.0" \
    "opentelemetry-exporter-otlp-proto-http==1.29.0"

# Keep the same repo-relative layout used in local development so the service
# can import generated source-of-truth NATS subject constants without pip
# packaging or mounting the host checkout at runtime.
COPY remote/deployments/ai-ml-pipeline/src ./src
COPY remote/deployments/ai-ml-pipeline/generated ./generated
COPY remote/libs/nats/subject-defs/generated/python /opt/dd-next-1/remote/libs/nats/subject-defs/generated/python

USER 10001:10001

ENV HOME=/tmp
ENV HOST=0.0.0.0
ENV PORT=8099
ENV PYTHONDONTWRITEBYTECODE=1

ENTRYPOINT ["python3", "-u", "/opt/dd-next-1/remote/deployments/ai-ml-pipeline/src/dd_ai_ml_pipeline.py"]
