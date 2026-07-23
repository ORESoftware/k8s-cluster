FROM docker.io/library/rust:1.88-alpine3.22 AS toolchain
RUN apk add --no-cache nodejs musl-dev ca-certificates \
  && addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/scintilla
COPY child-runtimes/polyglot-function-runner.mjs ./runner.mjs
COPY --chmod=0555 runtime-images/scintilla-entrypoint.sh /usr/local/bin/scintilla-entrypoint

FROM docker.io/library/alpine:3.22 AS runtime
RUN apk add --no-cache ca-certificates libgcc \
  && addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/scintilla
COPY --chmod=0555 runtime-images/scintilla-entrypoint.sh /usr/local/bin/scintilla-entrypoint
ENV HOME=/work TMPDIR=/work
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/scintilla-entrypoint"]
CMD ["/opt/scintilla/function"]

FROM toolchain AS dynamic
ENV LAMBDA_TARGET_RUNTIME=rust HOME=/work TMPDIR=/work
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/scintilla-entrypoint"]
CMD ["node", "/opt/scintilla/runner.mjs"]
