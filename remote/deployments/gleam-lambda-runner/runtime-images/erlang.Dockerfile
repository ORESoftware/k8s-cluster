FROM docker.io/library/erlang:28-alpine
RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  nodejs-current \
  && addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/dd-lambda
COPY child-runtimes/polyglot-function-runner.mjs ./runner.mjs
ENV LAMBDA_TARGET_RUNTIME=erlang
USER 10001:10001
ENTRYPOINT ["node", "/opt/dd-lambda/runner.mjs"]
