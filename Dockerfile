# syntax=docker/dockerfile:1
# Static Astro site image.
FROM node:26-slim@sha256:715e55e4b84e4bb0ff48e49b398a848f08e55daed8eb6a0ea1839ae53bc57583 AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git

ARG INTERFACES_REF=487e470c45ab5851e8f6f3b1dc048fe067fbf408
ARG TEST_CONFIG_REF=825220281fdc16bbf47a035177001d2fe29bdabf
WORKDIR /build
# package-lock.json contains sibling file: dependencies. Fetch only the exact
# requested commits and verify that callers supplied full immutable SHAs.
RUN test "${#INTERFACES_REF}" -eq 40 \
    && test -z "$(printf '%s' "$INTERFACES_REF" | tr -d '0-9a-f')" \
    && test "${#TEST_CONFIG_REF}" -eq 40 \
    && test -z "$(printf '%s' "$TEST_CONFIG_REF" | tr -d '0-9a-f')" \
    && git init --quiet fiducia-interfaces \
    && git -C fiducia-interfaces remote add origin https://github.com/fiducia-cloud/fiducia-interfaces.git \
    && git -C fiducia-interfaces fetch --quiet --depth=1 --no-tags origin "$INTERFACES_REF" \
    && git -C fiducia-interfaces checkout --quiet --detach FETCH_HEAD \
    && test "$(git -C fiducia-interfaces rev-parse HEAD)" = "$INTERFACES_REF" \
    && git init --quiet fiducia-test-config \
    && git -C fiducia-test-config remote add origin https://github.com/fiducia-cloud/fiducia-test-config.git \
    && git -C fiducia-test-config fetch --quiet --depth=1 --no-tags origin "$TEST_CONFIG_REF" \
    && git -C fiducia-test-config checkout --quiet --detach FETCH_HEAD \
    && test "$(git -C fiducia-test-config rev-parse HEAD)" = "$TEST_CONFIG_REF"

WORKDIR /build/fiducia-marketing.web
COPY package*.json ./
RUN npm ci --ignore-scripts
COPY . .
ARG PUBLIC_BASE=/fiducia
RUN PUBLIC_BASE="$PUBLIC_BASE" npm run build

FROM nginx:1.31-alpine@sha256:4a73073bd557c65b759505da037898b61f1be6cbcc3c2c3aeac22d2a470c1752
COPY nginx.conf /etc/nginx/nginx.conf
COPY --from=build --chown=nginx:nginx /build/fiducia-marketing.web/dist /srv/www
USER nginx
EXPOSE 8080
ENTRYPOINT ["nginx"]
CMD ["-g", "daemon off;"]
