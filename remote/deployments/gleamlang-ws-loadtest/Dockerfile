ARG FLAGS2ENV_REV=333c2ace93c362d274171ab0e0edf613d96a8e59

FROM ghcr.io/gleam-lang/gleam:v1.16.0-node-alpine AS flags2env-builder
ARG FLAGS2ENV_REV
WORKDIR /contract

RUN apk add --no-cache python3 make g++ \
 && npm install --global "https://github.com/flags-2-env/flags-2-env/archive/${FLAGS2ENV_REV}.tar.gz"

COPY .cli-flags.toml ./
RUN flags2env audit .cli-flags.toml \
 && resolved="$(flags2env __dd_gleamlang_ws_loadtest__ --client-count=7 --messages-per-second-per-client=2.5 --container-pool-timeout-ms=12345)" \
 && node -e 'const x=JSON.parse(process.argv[1]); if(x.CLIENT_COUNT!=="7"||x.MESSAGES_PER_SECOND_PER_CLIENT!=="2.5"||x.CONTAINER_POOL_TIMEOUT_MS!=="12345") process.exit(1)' "$resolved"

FROM ghcr.io/gleam-lang/gleam:v1.16.0-node-alpine
WORKDIR /app

COPY package.json ./
RUN npm install --omit=dev

COPY --from=flags2env-builder /usr/local/lib/node_modules/@oresoftware/f2e /usr/local/lib/node_modules/@oresoftware/f2e
COPY --from=flags2env-builder /usr/local/bin/flags2env /usr/local/bin/flags2env
COPY --from=flags2env-builder /usr/local/bin/f2e /usr/local/bin/f2e

COPY . .

RUN flags2env audit .cli-flags.toml

CMD ["gleam", "run", "--target", "javascript"]
