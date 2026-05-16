FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine
RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  nodejs-current \
  postgresql-client
WORKDIR /app
COPY . .

EXPOSE 8083
CMD ["gleam", "run"]
