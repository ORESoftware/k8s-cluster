FROM docker.io/library/node:22-alpine3.22@sha256:cd7807368cf24826297cbad5dca1a44972ccfd770647db52a8c7589eb4599ac8
RUN apk add --no-cache \
  chromium \
  ca-certificates \
  freetype \
  harfbuzz \
  nss \
  font-noto \
  && addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/dd-next
COPY deployments/gleam-lambda-runner/runtime-images/nodejs/package.json deployments/gleam-lambda-runner/runtime-images/nodejs/package-lock.json ./
RUN npm ci --omit=dev --ignore-scripts --no-audit --no-fund \
  && npm cache clean --force
COPY deployments/gleam-lambda-runner/child-runtimes/js-function-runner.mjs remote/deployments/gleam-lambda-runner/child-runtimes/js-function-runner.mjs
COPY libs/nats/subject-defs/generated/javascript/index.mjs remote/libs/nats/subject-defs/generated/javascript/index.mjs
ENV NODE_NO_WARNINGS=1 \
    LAMBDA_BROWSER_AUTOMATION=1 \
    LAMBDA_BROWSER_EXECUTABLE_PATH=/usr/bin/chromium-browser
USER 10001:10001
ENTRYPOINT ["node", "--permission", "--allow-child-process", "--allow-fs-read=/opt/dd-next", "--allow-fs-read=/usr/bin/chromium-browser", "--allow-fs-read=/usr/lib/chromium", "--allow-fs-read=/etc/fonts", "--allow-fs-read=/usr/share/fonts", "--allow-fs-write=/tmp", "/opt/dd-next/remote/deployments/gleam-lambda-runner/child-runtimes/js-function-runner.mjs"]
