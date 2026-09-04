# Linux AMD64 manifests are pinned because the current AWS worker is AMD64.
# The final published image must also be referenced by its immutable digest.
FROM docker.io/library/rust@sha256:408fe88047cef61a2087653b0c5255fa51c0f2d6d94ddedd7a2562a9b91a46f6 AS rust-toolchain

FROM ghcr.io/actions/actions-runner@sha256:91a87fc7ecea714252b01a1a3f0c64d0f8fe6a05fd466a631b9a51f88f4c7aca AS browser-assets

USER root

ARG CHROME_FOR_TESTING_VERSION=152.0.7977.64
ARG CHROME_FOR_TESTING_SHA256=8b592f066af71f054aab2cc80fc26f73c775c6d44ebb99d16ade924b24756c2e
ARG CHROMEDRIVER_SHA256=2457e3d1e204ca712d650e1f13c2b524270682471e371b4750fdbe4f15c1f2dc

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl unzip \
    && curl --fail --location --retry 3 \
        --output /tmp/chrome-linux64.zip \
        "https://storage.googleapis.com/chrome-for-testing-public/${CHROME_FOR_TESTING_VERSION}/linux64/chrome-linux64.zip" \
    && printf '%s  %s\n' "${CHROME_FOR_TESTING_SHA256}" /tmp/chrome-linux64.zip \
        | sha256sum --check --strict \
    && curl --fail --location --retry 3 \
        --output /tmp/chromedriver-linux64.zip \
        "https://storage.googleapis.com/chrome-for-testing-public/${CHROME_FOR_TESTING_VERSION}/linux64/chromedriver-linux64.zip" \
    && printf '%s  %s\n' "${CHROMEDRIVER_SHA256}" /tmp/chromedriver-linux64.zip \
        | sha256sum --check --strict \
    && install -d /opt/chrome-for-testing \
    && unzip -q /tmp/chrome-linux64.zip -d /opt/chrome-for-testing \
    && unzip -q /tmp/chromedriver-linux64.zip -d /opt/chrome-for-testing \
    && apt-get clean

FROM ghcr.io/actions/actions-runner@sha256:91a87fc7ecea714252b01a1a3f0c64d0f8fe6a05fd466a631b9a51f88f4c7aca

USER root

COPY --from=rust-toolchain /usr/local/cargo /usr/local/cargo
COPY --from=rust-toolchain /usr/local/rustup /usr/local/rustup
COPY --from=browser-assets /opt/chrome-for-testing /opt/chrome-for-testing

ENV CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    PATH=/usr/local/cargo/bin:${PATH} \
    CHROME_BIN=/usr/local/bin/chromium \
    PLAYWRIGHT_CHROMIUM=/usr/local/bin/chromium \
    CHROME_PATH=/usr/local/bin/chromium \
    PUPPETEER_EXECUTABLE_PATH=/usr/local/bin/chromium \
    PUPPETEER_SKIP_DOWNLOAD=1 \
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        git \
        jq \
        libasound2t64 \
        libatk-bridge2.0-0 \
        libatk1.0-0 \
        libatspi2.0-0 \
        libcups2t64 \
        libdbus-1-3 \
        libdrm2 \
        libgbm1 \
        libglib2.0-0 \
        libgtk-3-0 \
        libnspr4 \
        libnss3 \
        libpango-1.0-0 \
        libx11-6 \
        libxcomposite1 \
        libxdamage1 \
        libxext6 \
        libxfixes3 \
        libxkbcommon0 \
        libxrandr2 \
        libssl-dev \
        pkg-config \
        python3 \
        unzip \
        zip \
    && apt-get clean \
    && ln -s /opt/chrome-for-testing/chrome-linux64/chrome /usr/local/bin/chromium \
    && ln -s /opt/chrome-for-testing/chromedriver-linux64/chromedriver /usr/local/bin/chromedriver \
    && chown -R 1001:1001 /usr/local/cargo /usr/local/rustup

USER runner

RUN test "$(id -u)" = 1001 \
    && chromium --version \
    && chromedriver --version \
    && cargo --version \
    && rustup --version
