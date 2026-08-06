# syntax=docker/dockerfile:1
# Static Astro site image. Base images are digest-pinned so a registry tag move
# cannot silently change the reviewed build or runtime inputs.
FROM node:22-slim@sha256:f32b81066cde10a75dbac96646099533316d94bac4150c55da1636e1f0ffdc46 AS build
WORKDIR /app
COPY package*.json ./
# Playwright and Puppeteer are test-only dependencies. Their install hooks must
# not download hundreds of megabytes of browser binaries into the production
# build stage; CI installs the reviewed browser separately for e2e jobs.
ENV PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 \
    PUPPETEER_SKIP_DOWNLOAD=true
RUN npm ci
COPY . .
RUN npm run build

FROM nginx:1.27-alpine@sha256:65645c7bb6a0661892a8b03b89d0743208a18dd2f3f17a54ef4b76fb8e2f2a10
COPY --from=build /app/dist /usr/share/nginx/html
COPY nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
