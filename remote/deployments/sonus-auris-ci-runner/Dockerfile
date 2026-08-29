# syntax=docker/dockerfile:1.7

# GitHub-hosted self-runner version pinned to a concrete upstream release.
# Keep this at or above GitHub's enforced minimum and rebuild regularly.
ARG RUNNER_VERSION=2.334.0
FROM ghcr.io/actions/actions-runner:${RUNNER_VERSION}

USER root

# Non-privileged Linux CI/browser lane only. Rust, Node, Dart, Flutter and Java
# versions remain selected by repository workflows through pinned setup actions.
# The image supplies deterministic compiler, desktop, and browser libraries;
# Playwright/Puppeteer install their lockfile-selected browser binaries per job.
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      build-essential \
      ca-certificates \
      clang \
      cmake \
      curl \
      fonts-liberation \
      git \
      jq \
      libasound2 \
      libasound2-dev \
      libatk-bridge2.0-0 \
      libatk1.0-0 \
      libcups2 \
      libdbus-1-3 \
      libdrm2 \
      libgbm1 \
      libgtk-3-0 \
      libgtk-3-dev \
      liblzma-dev \
      libnspr4 \
      libnss3 \
      libpango-1.0-0 \
      libssl-dev \
      libudev-dev \
      libx11-xcb1 \
      libxcb1 \
      libxcomposite1 \
      libxdamage1 \
      libxext6 \
      libxfixes3 \
      libxkbcommon0 \
      libxrandr2 \
      ninja-build \
      openjdk-17-jdk-headless \
      pkg-config \
      python3 \
      python3-pip \
      unzip \
      xdg-utils \
      xz-utils \
      zip \
    && apt-get clean

# The official runner image uses uid/gid 1001. All writable paths are backed by
# bounded emptyDir volumes in the ARC pod template.
RUN install -d -o 1001 -g 1001 /home/runner/_work \
    && install -d -o 1001 -g 1001 /opt/hostedtoolcache \
    && install -d -o 1001 -g 1001 /tmp/sonus-ci

ENV JAVA_HOME=/usr/lib/jvm/java-17-openjdk-amd64 \
    PLAYWRIGHT_BROWSERS_PATH=/home/runner/.cache/ms-playwright

LABEL org.opencontainers.image.source="https://github.com/ORESoftware/k8s-cluster" \
      org.opencontainers.image.title="Sonus Auris non-privileged CI runner" \
      org.opencontainers.image.description="Ephemeral ARC runner for Rust, Node, Dart/Flutter web/Linux and browser checks" \
      org.opencontainers.image.licenses="MIT"

USER 1001
WORKDIR /home/runner
