FROM docker.io/library/node:22-alpine
RUN addgroup -S lambda && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/dd-lambda
COPY child-runtimes/js-function-runner.mjs ./runner.mjs
USER 10001:10001
ENTRYPOINT ["node", "--permission", "--allow-net", "/opt/dd-lambda/runner.mjs"]
