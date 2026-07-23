FROM docker.io/library/alpine:3.22
RUN apk add --no-cache \
  nodejs \
  bash \
  && addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/scintilla
COPY child-runtimes/bash-function-runner.mjs ./runner.mjs
COPY --chmod=0555 runtime-images/scintilla-entrypoint.sh /usr/local/bin/scintilla-entrypoint
ENV NODE_NO_WARNINGS=1 HOME=/work TMPDIR=/work
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/scintilla-entrypoint"]
CMD ["node", "--permission", "--allow-child-process", "/opt/scintilla/runner.mjs"]
