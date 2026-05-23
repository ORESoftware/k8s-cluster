# Multi-stage build for dd-dart-server.
#
# Stage 1 (flutter-build): uses the cirruslabs Flutter SDK image to compile
#   the desktop-shaped Flutter web SPA at /dart/app/ to JavaScript with
#   the offline-first PWA strategy. `flutter build web` emits everything
#   into build/web — including the service worker (`flutter_service_worker.js`)
#   and the bootstrapper (`flutter_bootstrap.js`).
#
# Stage 1b (flutter-mobile-build): identical shape, separate Flutter project
#   at flutter_mobile_app/, compiled with --base-href=/dart/mobile/ for the
#   mobile-optimized landing surface served at /dart/mobile/.
#
# Stage 2 (dart-build): uses the official Dart SDK image to AOT-compile the
#   server entry point and run jaspr_builder so `lib/jaspr_options.dart`
#   exists. The result is a self-contained native executable.
#
# Stage 3 (runtime): Debian-slim with glibc + ca-certificates + tzdata.
#   The compiled server binary plus both Flutter web bundles are copied in
#   (public/ for /dart/app/, mobile-public/ for /dart/mobile/), and the
#   container runs as an unprivileged user.
#
# Build from the repo root:
#   docker build \
#     -f remote/deployments/dart-server/Dockerfile \
#     -t dd-dart-server:dev .

# -- Stage 1: Flutter web build ----------------------------------------------
FROM ghcr.io/cirruslabs/flutter:3.44.0 AS flutter-build

WORKDIR /workspace
COPY remote/deployments/dart-server/flutter_app /workspace/flutter_app

WORKDIR /workspace/flutter_app
RUN flutter --disable-analytics
# `flutter create .` restores any web template files that aren't checked
# into the repo (icons, favicon, flutter_bootstrap stub) without touching
# pubspec.yaml or our hand-authored sources.
RUN flutter create --project-name dd_dart_flutter_app --platforms=web --org dev.dd .
RUN flutter pub get

# Vendor htmx + ws ext so the Flutter PWA still works offline. The
# bootstrapper inside index.html references these from /dart/app/vendor/.
RUN mkdir -p web/vendor && \
    curl -fsSL https://unpkg.com/htmx.org@2.0.6/dist/htmx.min.js -o web/vendor/htmx.min.js && \
    curl -fsSL https://unpkg.com/htmx-ext-ws@2.0.4/ws.js -o web/vendor/htmx-ext-ws.min.js

RUN flutter build web \
      --release \
      --pwa-strategy=offline-first \
      --base-href=/dart/app/

# -- Stage 1b: Flutter mobile web build --------------------------------------
# Independent Flutter web bundle, served at /dart/mobile/. Mirrors the
# /dart/app/ build above so both bundles ship side-by-side in the
# runtime image at /opt/dd-dart-server/{public,mobile-public}.
FROM ghcr.io/cirruslabs/flutter:3.44.0 AS flutter-mobile-build

WORKDIR /workspace
COPY remote/deployments/dart-server/flutter_mobile_app /workspace/flutter_mobile_app

WORKDIR /workspace/flutter_mobile_app
RUN flutter --disable-analytics
RUN flutter create --project-name dd_dart_flutter_mobile_app --platforms=web --org dev.dd .
RUN flutter pub get

RUN flutter build web \
      --release \
      --pwa-strategy=offline-first \
      --base-href=/dart/mobile/

# -- Stage 2: Dart server AOT build ------------------------------------------
FROM dart:stable AS dart-build

# Mirror the repo layout so `path:` dependencies in pubspec.yaml resolve
# the same way they do on a local checkout. The Dart server has a
# path-dependency on the canonical pg-defs Dart adapter via
# `../../libs/pg-defs/generated/dart`, so we layer that folder in too.
# Other Dart-flavoured services in this monorepo follow the same shape
# (gleamlang-ws-server / gleam-mcp-server with `generated/gleam`).
WORKDIR /workspace

COPY remote/libs/pg-defs/generated/dart \
     /workspace/remote/libs/pg-defs/generated/dart

COPY remote/deployments/dart-server/pubspec.yaml \
     /workspace/remote/deployments/dart-server/pubspec.yaml
COPY remote/deployments/dart-server/analysis_options.yaml \
     /workspace/remote/deployments/dart-server/analysis_options.yaml

WORKDIR /workspace/remote/deployments/dart-server
RUN dart pub get

COPY remote/deployments/dart-server/bin ./bin
COPY remote/deployments/dart-server/lib ./lib

# Generate jaspr_options.dart. We don't have any @client components, so the
# generated file is essentially `JasprOptions(targets: {}, clients: {})`,
# but Jaspr.initializeApp() still needs the symbol to resolve.
RUN dart run build_runner build --delete-conflicting-outputs

RUN dart compile exe bin/server.dart -o /tmp/dd-dart-server

# -- Stage 3: runtime --------------------------------------------------------
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 dart \
    && useradd  --system --uid 10001 --gid 10001 --home /opt/dd-dart-server dart

WORKDIR /opt/dd-dart-server

COPY --from=dart-build           /tmp/dd-dart-server                          /opt/dd-dart-server/dd-dart-server
COPY --from=flutter-build        /workspace/flutter_app/build/web             /opt/dd-dart-server/public
COPY --from=flutter-mobile-build /workspace/flutter_mobile_app/build/web      /opt/dd-dart-server/mobile-public

RUN chown -R dart:dart /opt/dd-dart-server

USER 10001:10001

ENV HTTP_HOST=0.0.0.0 \
    HTTP_PORT=8089 \
    STATIC_DIR=/opt/dd-dart-server/public \
    MOBILE_STATIC_DIR=/opt/dd-dart-server/mobile-public

EXPOSE 8089

ENTRYPOINT ["/opt/dd-dart-server/dd-dart-server"]
