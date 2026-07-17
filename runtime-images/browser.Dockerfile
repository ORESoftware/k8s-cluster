# Browser-automation runtime image for the `browser` lambda runtime
# (Playwright + Puppeteer). Unlike the other runtime images this is NOT Alpine:
# Chromium's shared libraries are painful on musl, so we start from the official
# Playwright image, which already ships Chromium and every system dependency it
# needs under /ms-playwright.
#
# Pin the Playwright image tag and the `playwright` npm version to the SAME
# release — browser builds and the driver are only guaranteed to match within a
# version. Bump both together.
FROM mcr.microsoft.com/playwright:v1.56.0-noble@sha256:35246d87a7c88ea9b771c65d33171b2611b02a8253b4b12ce6f94376c55f99f2

# Runner + libraries live here. `type: module` so the .mjs runner's ESM imports
# resolve `playwright` / `puppeteer` from this local node_modules.
WORKDIR /opt/dd-lambda

ENV NODE_ENV=production \
    NODE_NO_WARNINGS=1 \
    PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 \
    PUPPETEER_CACHE_DIR=/opt/dd-lambda/.puppeteer

COPY child-runtimes/browser-function-runner.mjs ./runner.mjs

# Install both APIs plus the standards-aware robots parser. Puppeteer uses the
# Playwright image's pinned Chromium binary, so no second browser is downloaded.
RUN printf '{"name":"dd-lambda-browser","private":true,"type":"module"}\n' > package.json \
 && npm install --no-save --omit=dev --ignore-scripts --package-lock=false \
      playwright@1.56.0 puppeteer-core@24.43.1 robots-parser@3.0.1 \
 # Non-root user matching the engine's `--user 10001:10001`. Everything the
 # runner writes at runtime (browser profiles, caches) must be owned by it, and
 # the rootfs is mounted read-only in prod so those dirs are the writable ones.
 && groupadd -g 10001 lambda \
 && useradd -u 10001 -g lambda -m -d /home/lambda lambda \
 && mkdir -p /opt/dd-lambda/.puppeteer /home/lambda \
 && chown -R 10001:10001 /opt/dd-lambda /home/lambda /ms-playwright

USER 10001:10001

# Line-delimited JSON over stdio (see browser-function-runner.mjs). No
# `--permission`: a browser needs the filesystem and child processes, so
# isolation is provided by the hardened, read-only, cap-dropped container the
# engine launches this in — not the Node permission model.
ENTRYPOINT ["node", "/opt/dd-lambda/runner.mjs"]
