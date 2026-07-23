FROM docker.io/library/python:3.12-alpine
RUN addgroup -S lambda && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/scintilla
COPY child-runtimes/python-function-runner.py ./runner.py
COPY --chmod=0555 runtime-images/scintilla-entrypoint.sh /usr/local/bin/scintilla-entrypoint
ENV HOME=/work TMPDIR=/work PYTHONUNBUFFERED=1
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/scintilla-entrypoint"]
CMD ["python3", "/opt/scintilla/runner.py"]
