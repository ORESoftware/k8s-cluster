FROM docker.io/library/ruby:3.3-alpine
RUN addgroup -S lambda && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/scintilla
COPY child-runtimes/ruby-function-runner.rb ./runner.rb
COPY --chmod=0555 runtime-images/scintilla-entrypoint.sh /usr/local/bin/scintilla-entrypoint
ENV HOME=/work TMPDIR=/work
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/scintilla-entrypoint"]
CMD ["ruby", "/opt/scintilla/runner.rb"]
