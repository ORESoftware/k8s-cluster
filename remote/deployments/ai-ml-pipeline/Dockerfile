FROM docker.io/library/python:3.12-slim

RUN groupadd --system pipeline && useradd --system --gid pipeline --uid 10001 pipeline
WORKDIR /opt/dd-ai-ml-pipeline
COPY src/dd_ai_ml_pipeline.py ./src/dd_ai_ml_pipeline.py
USER 10001:10001

ENV HOST=0.0.0.0
ENV PORT=8099
ENTRYPOINT ["python3", "-u", "/opt/dd-ai-ml-pipeline/src/dd_ai_ml_pipeline.py"]
