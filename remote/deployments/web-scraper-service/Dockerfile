# syntax=docker/dockerfile:1.7

ARG PLAYWRIGHT_VERSION=1.56.0
ARG FLAGS2ENV_REV=333c2ace93c362d274171ab0e0edf613d96a8e59
ARG TJSV_REV=8bf7ebda4e35c0d11b5b5e5f865867bee4fc3085
ARG TYPESPEC_VERSION=1.15.0
ARG TYPESPEC_ASSET_EMITTER_VERSION=0.79.1

FROM mcr.microsoft.com/playwright:v${PLAYWRIGHT_VERSION}-noble AS builder
ARG FLAGS2ENV_REV
ARG TJSV_REV
ARG TYPESPEC_VERSION
ARG TYPESPEC_ASSET_EMITTER_VERSION

# Build from the repository root because @dd/telemetry is a pinned file
# dependency in the private remote/libs submodule:
#   docker build -f remote/deployments/web-scraper-service/Dockerfile \
#     -t dd-web-scraper:dev .
WORKDIR /app/remote/deployments/web-scraper-service

ENV FLAGS2ENV_HOME=/opt/flags2env \
    PATH=/opt/flags2env/bin:${PATH} \
    PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 \
    PUPPETEER_SKIP_DOWNLOAD=true

# The pinned flags2env package builds its Node binding with node-gyp during
# installation. Keep compiler tooling in the builder stage only so the final
# scraper runtime remains unchanged and does not gain a compiler toolchain.
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential \
 && rm -rf /var/lib/apt/lists/*

# npm's default global prefix differs across base images and Corepack setups.
# Use an explicit application-owned prefix so the exact tested flags2env binary
# and native binding can be copied into the runtime image deterministically.
RUN corepack enable \
 && corepack prepare pnpm@9.15.4 --activate \
 && pnpm --version \
 && npm install --global --prefix "${FLAGS2ENV_HOME}" \
      "https://github.com/flags-2-env/flags-2-env/archive/${FLAGS2ENV_REV}.tar.gz" \
 && flags2env --version

COPY remote/libs/telemetry-node /app/remote/libs/telemetry-node
COPY remote/deployments/web-scraper-service/package.json \
     remote/deployments/web-scraper-service/pnpm-lock.yaml \
     remote/deployments/web-scraper-service/tsconfig.json \
     remote/deployments/web-scraper-service/.cli-flags.toml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store,sharing=locked \
    pnpm install --frozen-lockfile

# TJSV invokes the TypeSpec compiler with the authored contract directory as
# its cwd. Install the exact compiler/emitter pair at the scraper project root
# so `import "@typespec/json-schema"` resolves by ordinary Node/TypeSpec package
# lookup from contracts/*.tsp. The upstream TJSV lock also pins asset-emitter
# 0.79.1; keep that exact transitive version so a fresh pnpm consumer does not
# float to 0.79.2, whose compiler peer requires ^1.16.0.
RUN --mount=type=cache,target=/root/.local/share/pnpm/store,sharing=locked \
    pnpm add --save-dev --lockfile=false \
      "https://github.com/ORESoftware/typespec-json-schema-validator/archive/${TJSV_REV}.tar.gz" \
      "@typespec/compiler@${TYPESPEC_VERSION}" \
      "@typespec/json-schema@${TYPESPEC_VERSION}" \
      "@typespec/asset-emitter@${TYPESPEC_ASSET_EMITTER_VERSION}"

COPY remote/deployments/web-scraper-service/contracts ./contracts
COPY remote/deployments/web-scraper-service/src ./src
COPY remote/deployments/web-scraper-service/tests ./tests
RUN pnpm run build \
 && flags2env audit .cli-flags.toml \
 && APIFY_FALLBACK_ENABLED=true flags2env __dd_web_scraper__ --port=8098 >/tmp/dd-web-scraper-flags.json \
 && node -e "const fs=require('node:fs');const v=JSON.parse(fs.readFileSync('/tmp/dd-web-scraper-flags.json','utf8'));if(v.PORT!=='8098'||v.APIFY_FALLBACK_ENABLED!=='true')process.exit(1)" \
 && pnpm exec tjsv check \
      --typespec=contracts/scraper-fallback.tsp \
      --schema=contracts/scraper-fallback.schema.json \
      --instances=contracts/instances \
      --report=/tmp/dd-web-scraper-contract-report.json \
      --quiet \
 && node --import tsx --test \
      tests/apify-fallback.test.ts \
      tests/flags-runtime.test.ts \
      tests/runtime-config-guard.test.ts \
      tests/scraper-contract-fixtures.test.ts \
      tests/sensitive-field-policy.test.ts \
      tests/sensitive-field-wiring.test.ts \
      tests/sensitive-field-policy.integration-plan.test.ts \
 && pnpm prune --prod

FROM mcr.microsoft.com/playwright:v${PLAYWRIGHT_VERSION}-noble AS runtime

WORKDIR /app/remote/deployments/web-scraper-service

ENV FLAGS2ENV_HOME=/opt/flags2env \
    PATH=/opt/flags2env/bin:${PATH} \
    NODE_ENV=production \
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
COPY --from=builder /app/remote/deployments/web-scraper-service/.cli-flags.toml ./
COPY --from=builder /app/remote/deployments/web-scraper-service/node_modules ./node_modules
COPY --from=builder /app/remote/deployments/web-scraper-service/dist ./dist
COPY --from=builder /opt/flags2env /opt/flags2env

EXPOSE 8097

USER pwuser
CMD ["node", "dist/scraper-entrypoint.js"]
