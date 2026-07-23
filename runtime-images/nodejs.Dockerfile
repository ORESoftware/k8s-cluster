FROM docker.io/library/node:22-alpine3.22@sha256:cd7807368cf24826297cbad5dca1a44972ccfd770647db52a8c7589eb4599ac8 AS toolchain
WORKDIR /opt/scintilla
COPY deployments/scintilla-run-monorepo/apps/gleam-lambda-runner/runtime-images/nodejs/package.json deployments/scintilla-run-monorepo/apps/gleam-lambda-runner/runtime-images/nodejs/package-lock.json ./
RUN npm ci --omit=dev --ignore-scripts --no-audit --no-fund \
  && npm cache clean --force

# Minimal base for a prebuilt/custom Node function image. A downstream build
# copies its protocol-speaking command to /opt/scintilla/function.mjs.
FROM docker.io/library/node:22-alpine3.22@sha256:cd7807368cf24826297cbad5dca1a44972ccfd770647db52a8c7589eb4599ac8 AS runtime
RUN apk add --no-cache ca-certificates \
  && addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/scintilla
COPY --chmod=0555 deployments/scintilla-run-monorepo/apps/gleam-lambda-runner/runtime-images/scintilla-entrypoint.sh /usr/local/bin/scintilla-entrypoint
ENV NODE_ENV=production HOME=/work TMPDIR=/work
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/scintilla-entrypoint"]
CMD ["node", "/opt/scintilla/function.mjs"]

# Compiler/interpreter-equipped image used for source definitions stored by
# Scintilla. This is the default final target published by GitOps.
FROM runtime AS dynamic
USER root
RUN apk add --no-cache \
  chromium \
  freetype \
  harfbuzz \
  nss \
  font-noto
COPY --from=toolchain /opt/scintilla/node_modules /opt/scintilla/node_modules
COPY deployments/scintilla-run-monorepo/apps/gleam-lambda-runner/child-runtimes/js-function-runner.mjs /opt/dd-next/remote/deployments/scintilla-run-monorepo/apps/gleam-lambda-runner/child-runtimes/js-function-runner.mjs
COPY libs/nats/subject-defs/generated/javascript/index.mjs /opt/dd-next/remote/libs/nats/subject-defs/generated/javascript/index.mjs
ENV NODE_NO_WARNINGS=1 \
    LAMBDA_BROWSER_AUTOMATION=1 \
    LAMBDA_BROWSER_EXECUTABLE_PATH=/usr/bin/chromium-browser
USER 10001:10001
CMD ["node", "--permission", "--allow-child-process", "--allow-fs-read=/opt/dd-next", "--allow-fs-read=/usr/bin/chromium-browser", "--allow-fs-read=/usr/lib/chromium", "--allow-fs-read=/etc/fonts", "--allow-fs-read=/usr/share/fonts", "--allow-fs-read=/tmp", "--allow-fs-read=/work", "--allow-fs-write=/tmp", "--allow-fs-write=/work", "/opt/dd-next/remote/deployments/scintilla-run-monorepo/apps/gleam-lambda-runner/child-runtimes/js-function-runner.mjs"]
