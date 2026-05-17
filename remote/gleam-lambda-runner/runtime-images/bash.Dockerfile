FROM docker.io/library/alpine:edge
RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  nodejs-current \
  bash \
  && addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/dd-lambda
COPY child-runtimes/bash-function-runner.mjs ./runner.mjs
USER 10001:10001
ENTRYPOINT ["node", "--permission", "--allow-net", "--allow-child-process", "/opt/dd-lambda/runner.mjs"]
