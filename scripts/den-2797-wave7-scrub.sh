#!/usr/bin/env bash
set +e
shred -u \
  /tmp/den-2797-wave7-private.pem \
  /tmp/den-2797-wave7-public.pem \
  /tmp/den-2797-wave7-token.enc \
  /tmp/den-2797-wave7-token.txt \
  /tmp/den-2797-wave7-askpass \
  2>/dev/null
rm -rf /tmp/den-2797-wave7
