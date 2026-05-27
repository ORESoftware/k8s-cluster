#!/usr/bin/env bash
# Periodic reaper: kill leaked `nerdctl ... internal oci-hook postStop`
# processes (nerdctl 2.0.2 bug: when runc init hangs, the postStop hook leaks
# and accumulates one process per failed container start), and any dd-pool
# tasks stuck in UNKNOWN state for long enough that we are confident the
# shim is hung and the runtime entry is dead metadata.
#
# Driven by /etc/systemd/system/dd-nerdctl-hook-reaper.timer, every 60 s.
set +e

NOW=$(date -u +%s)
MAX_HOOK_AGE_SECONDS=${MAX_HOOK_AGE_SECONDS:-90}
MAX_UNKNOWN_AGE_SECONDS=${MAX_UNKNOWN_AGE_SECONDS:-300}

killed_hooks=0
while IFS= read -r pid; do
  [ -z "$pid" ] && continue
  start_s=$(stat -c %Y /proc/"$pid" 2>/dev/null || echo 0)
  age=$(( NOW - start_s ))
  if [ "$age" -ge "$MAX_HOOK_AGE_SECONDS" ]; then
    kill -KILL "$pid" 2>/dev/null && killed_hooks=$(( killed_hooks + 1 ))
  fi
done < <(pgrep -f 'nerdctl .*internal oci-hook' 2>/dev/null)

if [ "$killed_hooks" -gt 0 ]; then
  logger -t dd-nerdctl-hook-reaper \
    "killed=$killed_hooks stale oci-hook procs (age>=${MAX_HOOK_AGE_SECONDS}s)"
fi

# Any dd-pool task that's been in UNKNOWN for > 5 min is presumed wedged. Force
# delete it so container-pool can re-create the warm worker on the next pull.
for c in $(ctr -n dd-pool tasks ls 2>/dev/null | awk '$3 == "UNKNOWN" {print $1}'); do
  container_dir="/var/lib/containerd/io.containerd.runtime.v2.task/dd-pool/$c"
  if [ -d "$container_dir" ]; then
    start_s=$(stat -c %Y "$container_dir" 2>/dev/null || echo 0)
    age=$(( NOW - start_s ))
    if [ "$age" -ge "$MAX_UNKNOWN_AGE_SECONDS" ]; then
      ctr -n dd-pool tasks delete --force "$c" 2>/dev/null
      ctr -n dd-pool containers delete "$c" 2>/dev/null
      logger -t dd-nerdctl-hook-reaper \
        "reaped dd-pool stuck container=$c age=${age}s"
    fi
  fi
done
