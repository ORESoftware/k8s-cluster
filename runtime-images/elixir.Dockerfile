FROM docker.io/library/elixir:1.18-alpine AS toolchain
RUN apk add --no-cache nodejs ca-certificates \
  && addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/scintilla
COPY child-runtimes/polyglot-function-runner.mjs ./runner.mjs
COPY --chmod=0555 runtime-images/scintilla-entrypoint.sh /usr/local/bin/scintilla-entrypoint

FROM docker.io/library/elixir:1.18-alpine AS runtime
RUN addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/scintilla
COPY --chmod=0555 runtime-images/scintilla-entrypoint.sh /usr/local/bin/scintilla-entrypoint
ENV MIX_ENV=prod HOME=/work TMPDIR=/work
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/scintilla-entrypoint"]
CMD ["elixir", "/opt/scintilla/function.exs"]

FROM toolchain AS dynamic
ENV LAMBDA_TARGET_RUNTIME=elixir MIX_ENV=prod HOME=/work TMPDIR=/work
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/scintilla-entrypoint"]
CMD ["node", "/opt/scintilla/runner.mjs"]
