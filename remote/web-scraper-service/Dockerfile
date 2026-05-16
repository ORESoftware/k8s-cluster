# syntax=docker/dockerfile:1.7

ARG PLAYWRIGHT_VERSION=1.56.0

FROM mcr.microsoft.com/playwright:v${PLAYWRIGHT_VERSION}-noble AS builder
WORKDIR /srv/web-scraper

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
WORKDIR /srv/web-scraper

ENV NODE_ENV=production \
    HOST=0.0.0.0 \
    PORT=8097 \
    PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 \
    PUPPETEER_SKIP_DOWNLOAD=true

RUN corepack enable \
 && corepack prepare pnpm@9.15.4 --activate

COPY --from=builder /srv/web-scraper/package.json /srv/web-scraper/pnpm-lock.yaml ./
COPY --from=builder /srv/web-scraper/node_modules ./node_modules
COPY --from=builder /srv/web-scraper/dist ./dist

EXPOSE 8097

CMD ["node", "dist/server.js"]
