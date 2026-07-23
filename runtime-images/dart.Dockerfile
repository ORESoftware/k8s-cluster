FROM docker.io/library/dart:stable
RUN apt-get update \
  && apt-get install -y --no-install-recommends nodejs ca-certificates \
  && groupadd --system lambda \
  && useradd --system --gid lambda --uid 10001 --create-home lambda \
  && apt-get clean
WORKDIR /opt/dd-lambda
COPY child-runtimes/polyglot-function-runner.mjs ./runner.mjs
COPY --chmod=0555 runtime-images/scintilla-entrypoint.sh /usr/local/bin/scintilla-entrypoint
ENV LAMBDA_TARGET_RUNTIME=dart HOME=/work TMPDIR=/work
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/scintilla-entrypoint"]
CMD ["node", "/opt/dd-lambda/runner.mjs"]
