#!/usr/bin/env bash
# Mirror the worker image (`dd-dev-server:dev`) from the `k8s.io` containerd
# namespace (where the daily nerdctl build job in idle-reaper-rs writes it)
# into `dd-pool`, which is the namespace dd-container-pool runs warm workers
# in. Without this timer the warm pool would only see new builds after the
# next `reconcile-runtime` GitHub Actions run; the timer keeps the lag
# bounded to about 10 minutes.
#
# Idempotent: imports are no-ops if the manifest already exists.
set +e

IMAGE=${WORKER_POOL_IMAGE:-docker.io/library/dd-dev-server:dev}
SRC_NS=${SRC_NAMESPACE:-k8s.io}
DST_NS=${DST_NAMESPACE:-dd-pool}

if ! ctr -n "$SRC_NS" images list --quiet "name==$IMAGE" 2>/dev/null | grep -q . ; then
  exit 0
fi

src_digest=$(ctr -n "$SRC_NS" images list "name==$IMAGE" 2>/dev/null \
  | awk 'NR==2 {print $3}')
dst_digest=$(ctr -n "$DST_NS" images list "name==$IMAGE" 2>/dev/null \
  | awk 'NR==2 {print $3}')

if [ -n "$src_digest" ] && [ "$src_digest" = "$dst_digest" ]; then
  exit 0
fi

tmp_tar="$(mktemp /tmp/dd-pool-image-sync.XXXXXX.tar)"
trap 'rm -f "$tmp_tar"' EXIT

if ! ctr -n "$SRC_NS" image export "$tmp_tar" "$IMAGE" 2>/dev/null; then
  logger -t dd-pool-image-sync "export from $SRC_NS failed for $IMAGE"
  exit 1
fi

if ! ctr -n "$DST_NS" image import "$tmp_tar" >/dev/null 2>&1; then
  logger -t dd-pool-image-sync "import into $DST_NS failed for $IMAGE"
  exit 1
fi

logger -t dd-pool-image-sync \
  "synced $IMAGE: $SRC_NS=$src_digest -> $DST_NS=$dst_digest"
