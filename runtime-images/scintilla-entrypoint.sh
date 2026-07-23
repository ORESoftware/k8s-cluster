#!/bin/sh
set -eu

# The parent runner passes an override as one environment value. It is never
# interpolated into the host command that launches this container. A custom
# command must implement Scintilla's line-delimited JSON protocol on stdin/stdout.
if [ -n "${SCINTILLA_RUNTIME_COMMAND:-}" ]; then
  exec /bin/sh -c "$SCINTILLA_RUNTIME_COMMAND"
fi

if [ "$#" -eq 0 ]; then
  echo "scintilla-entrypoint: no runtime command configured" >&2
  exit 64
fi

exec "$@"
