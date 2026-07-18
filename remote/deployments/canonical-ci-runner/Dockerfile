# Chromium-capable GitHub Actions runner image for the canonical.cloud browser
# e2e (Playwright + Puppeteer). Built on GitHub's official ARC runner image so
# the gha-runner-scale-set lifecycle (registration, job execution, ephemeral
# teardown) is unchanged; we only add Node, Chromium, and the browser system
# libraries so the jobs need no per-run browser download.
#
# Build & push (adjust the registry to wherever the cluster pulls from; the
# runner-set Application references ghcr.io/canonical-cloud/canonical-ci-runner):
#   docker build -t ghcr.io/canonical-cloud/canonical-ci-runner:browser \
#     remote/deployments/canonical-ci-runner
#   docker push ghcr.io/canonical-cloud/canonical-ci-runner:browser
FROM ghcr.io/actions/actions-runner:2.320.0

USER root

# Node 22 to match the app repos' CI (actions/setup-node node-version: "22"),
# Chromium, and the shared library set Chromium/Playwright need headless.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl gnupg \
    && mkdir -p /etc/apt/keyrings \
    && curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key \
        | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg \
    && echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_22.x nodistro main" \
        > /etc/apt/sources.list.d/nodesource.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        nodejs \
        chromium \
        fonts-liberation \
        libasound2 \
        libatk-bridge2.0-0 \
        libatk1.0-0 \
        libc6 \
        libcairo2 \
        libcups2 \
        libdbus-1-3 \
        libexpat1 \
        libgbm1 \
        libglib2.0-0 \
        libnspr4 \
        libnss3 \
        libpango-1.0-0 \
        libx11-6 \
        libxcb1 \
        libxcomposite1 \
        libxdamage1 \
        libxext6 \
        libxfixes3 \
        libxkbcommon0 \
        libxrandr2 \
    && rm -rf /var/lib/apt/lists/*

# The browser harnesses (chromeExecutablePath) read these to reuse the OS
# Chromium; also set as defaults here so ad-hoc jobs on this runner behave.
ENV PLAYWRIGHT_CHROMIUM=/usr/bin/chromium \
    CHROME_PATH=/usr/bin/chromium \
    PUPPETEER_EXECUTABLE_PATH=/usr/bin/chromium \
    PUPPETEER_SKIP_DOWNLOAD=1 \
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1

USER runner
