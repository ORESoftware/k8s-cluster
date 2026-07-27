# syntax=docker/dockerfile:1.7

ARG PLAYWRIGHT_VERSION=1.56.0

FROM mcr.microsoft.com/playwright:v${PLAYWRIGHT_VERSION}-noble AS builder

# Build from the repository root because @dd/telemetry is a pinned file
# dependency in the private remote/libs submodule:
#   docker build -f remote/deployments/web-scraper-service/Dockerfile \
#     -t dd-web-scraper:dev .
WORKDIR /app/remote/deployments/web-scraper-service

ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 \
    PUPPETEER_SKIP_DOWNLOAD=true

RUN corepack enable \
 && corepack prepare pnpm@9.15.4 --activate \
 && pnpm --version

COPY remote/libs/telemetry-node /app/remote/libs/telemetry-node
COPY remote/deployments/web-scraper-service/package.json \
     remote/deployments/web-scraper-service/pnpm-lock.yaml \
     remote/deployments/web-scraper-service/tsconfig.json ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store,sharing=locked \
    pnpm install --frozen-lockfile

COPY remote/deployments/web-scraper-service/src ./src
COPY remote/deployments/web-scraper-service/tests ./tests
RUN pnpm run build \
 && node --import tsx --test \
      tests/sensitive-field-policy.test.ts \
      tests/sensitive-field-wiring.test.ts \
      tests/sensitive-field-policy.integration-plan.test.ts \
 && pnpm prune --prod

FROM mcr.microsoft.com/playwright:v${PLAYWRIGHT_VERSION}-noble AS runtime

WORKDIR /app/remote/deployments/web-scraper-service

ENV NODE_ENV=production \
    HOST=0.0.0.0 \
    PORT=8097 \
    HOME=/tmp \
    XDG_CACHE_HOME=/tmp/.cache \
    PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 \
    PUPPETEER_SKIP_DOWNLOAD=true

# Keep the same filesystem relationship used while installing so pnpm's local
# @dd/telemetry link resolves without copying source or package-manager tooling
# into the runtime startup path.
COPY --from=builder /app/remote/libs/telemetry-node /app/remote/libs/telemetry-node
COPY --from=builder /app/remote/deployments/web-scraper-service/package.json ./
COPY --from=builder /app/remote/deployments/web-scraper-service/node_modules ./node_modules
COPY --from=builder /app/remote/deployments/web-scraper-service/dist ./dist

EXPOSE 8097

USER pwuser
CMD ["node", "dist/server.js"]
