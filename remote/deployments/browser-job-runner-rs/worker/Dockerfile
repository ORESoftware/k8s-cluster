# syntax=docker/dockerfile:1.7
#
# dd-browser-job-worker — the per-job container image spawned by
# dd-browser-job-runner. Built on the Playwright Noble image so Chromium plus
# its GUI libs are present for both Playwright and Puppeteer (which reuses the
# Playwright-bundled Chromium via executablePath). Build into the spawner's
# containerd namespace so `nerdctl run --pull=never` can find it, e.g.:
#
#     nerdctl -n dd-browser-jobs build \
#       -t docker.io/library/dd-browser-job-worker:dev \
#       remote/deployments/browser-job-runner-rs/worker

ARG PLAYWRIGHT_VERSION=1.56.0

FROM mcr.microsoft.com/playwright:v${PLAYWRIGHT_VERSION}-noble AS builder
WORKDIR /srv/browser-job-worker

ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 \
    PUPPETEER_SKIP_DOWNLOAD=true

RUN corepack enable \
 && corepack prepare pnpm@9.15.4 --activate \
 && pnpm --version

COPY package.json pnpm-lock.yaml* tsconfig.json ./
RUN if [ -f pnpm-lock.yaml ]; then pnpm install --frozen-lockfile; else pnpm install --no-frozen-lockfile; fi

COPY src ./src
RUN pnpm run build \
 && pnpm prune --prod

FROM mcr.microsoft.com/playwright:v${PLAYWRIGHT_VERSION}-noble AS runtime
WORKDIR /srv/browser-job-worker

ENV NODE_ENV=production \
    PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 \
    PUPPETEER_SKIP_DOWNLOAD=true

COPY --from=builder /srv/browser-job-worker/package.json ./
COPY --from=builder /srv/browser-job-worker/node_modules ./node_modules
COPY --from=builder /srv/browser-job-worker/dist ./dist

# Single-shot: the process runs one scenario, publishes to NATS, and exits.
CMD ["node", "dist/worker.js"]
