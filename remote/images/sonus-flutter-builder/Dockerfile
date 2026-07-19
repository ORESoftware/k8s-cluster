FROM ghcr.io/cirruslabs/android-sdk:36@sha256:f9b3ea9ed2b5fc9522adae82c7b4622ab7aa54207ef532c8e615a347dca08f31

USER root

ARG FLUTTER_REVISION=c9a6c484230f8b5e408ec57be1ef71dee1e77020
ARG FLUTTER_VERSION=3.44.2
ENV FLUTTER_HOME=/opt/flutter \
    FLUTTER_ROOT=/opt/flutter \
    HOME=/tmp \
    PATH=/opt/flutter/bin:/opt/flutter/bin/cache/dart-sdk/bin:${PATH}

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
       ca-certificates clang cmake curl git libglu1-mesa libgtk-3-dev liblzma-dev \
       ninja-build pkg-config unzip xz-utils zip

RUN git init "${FLUTTER_HOME}" \
    && git -C "${FLUTTER_HOME}" remote add origin https://github.com/flutter/flutter.git \
    && git -C "${FLUTTER_HOME}" fetch --depth 1 origin "${FLUTTER_REVISION}" \
    && git -C "${FLUTTER_HOME}" switch --detach FETCH_HEAD \
    && test "$(git -C "${FLUTTER_HOME}" rev-parse HEAD)" = "${FLUTTER_REVISION}" \
    && git -C "${FLUTTER_HOME}" tag "${FLUTTER_VERSION}" "${FLUTTER_REVISION}"

RUN flutter config --no-analytics \
    && yes | flutter doctor --android-licenses \
    && flutter precache --android --web --linux \
    && flutter doctor -v

LABEL org.opencontainers.image.source="https://github.com/ORESoftware/k8s-cluster" \
      org.opencontainers.image.description="Pinned Sonus Flutter Android/web/Linux builder" \
      dev.sonus-auris.flutter-revision="${FLUTTER_REVISION}" \
      dev.sonus-auris.flutter-version="${FLUTTER_VERSION}"
