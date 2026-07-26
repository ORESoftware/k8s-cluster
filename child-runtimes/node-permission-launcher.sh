#!/bin/sh
set -eu

# Node added network permissions in v25. Earlier releases reject --allow-net
# and leave network access outside the permission model. Feature detection
# keeps one runner command valid across supported host and container releases.
if node --help 2>&1 | grep -q -- '--allow-net'; then
  exec node --permission --allow-net "$@"
fi

exec node --permission "$@"
