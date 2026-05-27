# syntax=docker/dockerfile:1.7

ARG PLAYWRIGHT_VERSION=1.56.0

# The Playwright Noble image already ships with Chromium plus the GUI libs
# Selenium and Puppeteer need (libnss, libatk, libxkbcommon, etc.). We reuse
# that browser instead of installing google-chrome-stable so all three drivers
# share a single binary, which keeps the image small and version-coherent.

FROM mcr.microsoft.com/playwright:v${PLAYWRIGHT_VERSION}-noble AS builder
WORKDIR /srv/browser-test-server

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
WORKDIR /srv/browser-test-server

ENV NODE_ENV=production \
    HOST=0.0.0.0 \
    PORT=8104 \
    PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 \
    PUPPETEER_SKIP_DOWNLOAD=true

RUN corepack enable \
 && corepack prepare pnpm@9.15.4 --activate

COPY --from=builder /srv/browser-test-server/package.json /srv/browser-test-server/pnpm-lock.yaml ./
COPY --from=builder /srv/browser-test-server/node_modules ./node_modules
COPY --from=builder /srv/browser-test-server/dist ./dist

EXPOSE 8104

CMD ["node", "dist/server.js"]
