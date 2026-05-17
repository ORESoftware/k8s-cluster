FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine
WORKDIR /app
RUN apk add --no-cache nodejs
COPY . .

EXPOSE 8081
CMD ["gleam", "run"]
