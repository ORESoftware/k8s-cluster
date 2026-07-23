FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine AS toolchain
RUN apk add --no-cache nodejs ca-certificates \
  && addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/scintilla
COPY child-runtimes/polyglot-function-runner.mjs ./runner.mjs
COPY --chmod=0555 runtime-images/scintilla-entrypoint.sh /usr/local/bin/scintilla-entrypoint

FROM docker.io/library/erlang:28-alpine AS runtime
RUN addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/scintilla
COPY --chmod=0555 runtime-images/scintilla-entrypoint.sh /usr/local/bin/scintilla-entrypoint
ENV HOME=/work TMPDIR=/work
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/scintilla-entrypoint"]
CMD ["erl", "-noshell", "-pa", "/opt/scintilla/ebin", "-s", "scintilla_function@@main", "run", "-s", "init", "stop"]

FROM toolchain AS dynamic
ENV LAMBDA_TARGET_RUNTIME=gleam HOME=/work TMPDIR=/work
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/scintilla-entrypoint"]
CMD ["node", "/opt/scintilla/runner.mjs"]
