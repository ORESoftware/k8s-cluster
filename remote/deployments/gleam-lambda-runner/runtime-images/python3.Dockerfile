FROM docker.io/library/python:3.12-alpine
RUN addgroup -S lambda && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/dd-lambda
COPY child-runtimes/python-function-runner.py ./runner.py
USER 10001:10001
ENTRYPOINT ["python3", "/opt/dd-lambda/runner.py"]
