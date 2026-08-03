# syntax=docker/dockerfile:1.7
# Keep this at or above GitHub's enforced minimum self-hosted runner version.
ARG RUNNER_VERSION=2.334.0
FROM ghcr.io/actions/actions-runner:${RUNNER_VERSION}

USER root
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      build-essential \
      ca-certificates \
      clang \
      cmake \
      curl \
      git \
      git-lfs \
      jq \
      libssl-dev \
      openssh-client \
      pkg-config \
      python3 \
      python3-venv \
      unzip \
      zip \
 && apt-get clean \
 && rm -rf /var/lib/apt/lists/* \
 && install -d -o 1001 -g 1001 /home/runner/_work \
 && install -d -o 1001 -g 1001 /opt/hostedtoolcache

ENV HOME=/home/runner \
    RUNNER_TOOL_CACHE=/opt/hostedtoolcache \
    AGENT_TOOLSDIRECTORY=/opt/hostedtoolcache

LABEL org.opencontainers.image.source="https://github.com/ORESoftware/k8s-cluster" \
      org.opencontainers.image.title="ORESoftware non-privileged CI runner" \
      org.opencontainers.image.description="Ephemeral ARC runner base for repository-selected toolchains" \
      org.opencontainers.image.licenses="MIT"

USER 1001
WORKDIR /home/runner
