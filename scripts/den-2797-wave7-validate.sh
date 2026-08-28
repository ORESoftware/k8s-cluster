#!/usr/bin/env bash
set -Eeuo pipefail
test "$GITHUB_REPOSITORY" = ORESoftware/k8s-cluster
test "$GITHUB_REF" = refs/heads/main
test "$(git rev-parse HEAD)" = "$GITHUB_SHA"
test "$WORKFLOW_PATH" = .github/workflows/ops-den-2797-publish-wave7-recovery-once.yml
for command in base64 cargo curl gh git jq node npm openssl python3 rustup sha256sum shred unzip; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "$command is required" >&2
    exit 127
  }
done
case "$PUBLIC_KEY_PATH:$CIPHERTEXT_PATH" in
  .github/tmp/den-2797-wave7-*.pub.pem:.github/tmp/den-2797-wave7-*.enc.b64) ;;
  *) echo "invalid one-time handoff paths" >&2; exit 2 ;;
esac
