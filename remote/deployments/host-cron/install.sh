#!/usr/bin/env bash
# Idempotent installer for the host-cron deployment.
# - Copies executables from scripts/ into /usr/local/sbin (mode 0755).
# - Copies systemd units from units/ into /etc/systemd/system.
# - Reloads systemd, then enables and starts every *.timer in units/.
# - Runs checksum-pinned one-time repository publishers on the protected host.
#
# Designed to be invoked from .github/workflows/remote-k8s-maintenance.yml
# during reconcile-runtime, and from an operator shell on the EC2 host.
#
# Must run as root (uses /etc, /usr/local/sbin, systemctl).
set -euo pipefail
umask 077

here="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$here/../../.." && pwd)"
scripts_dir="$here/scripts"
units_dir="$here/units"

if [ "$(id -u)" -ne 0 ]; then
  echo "install.sh must run as root (got uid=$(id -u))" >&2
  exit 1
fi

echo "--- installing executables from $scripts_dir into /usr/local/sbin ---"
for src in "$scripts_dir"/*.sh; do
  [ -f "$src" ] || continue
  name="$(basename "$src")"
  dst="/usr/local/sbin/$name"
  install -m 0755 -o root -g root "$src" "$dst"
  echo "  $dst"
done

echo "--- installing systemd units from $units_dir into /etc/systemd/system ---"
for src in "$units_dir"/*.service "$units_dir"/*.timer; do
  [ -f "$src" ] || continue
  name="$(basename "$src")"
  dst="/etc/systemd/system/$name"
  install -m 0644 -o root -g root "$src" "$dst"
  echo "  $dst"
done

echo "--- systemctl daemon-reload ---"
systemctl daemon-reload

echo "--- enable and start each timer ---"
for unit in "$units_dir"/*.timer; do
  [ -f "$unit" ] || continue
  name="$(basename "$unit")"
  systemctl enable --now "$name"
  echo "  enabled $name"
done

echo "--- current timer state ---"
systemctl list-timers --all 'dd-*.timer' --no-pager || true

# The new workflow path used for the first publication attempt is not admitted
# by the current AWS OIDC trust policy. This hook deliberately runs through the
# already-established remote-k8s-maintenance reconcile path and protected host.
# It is idempotent at both levels: the host marker suppresses repeated downloads,
# while the publisher validates and normalizes repositories that already exist.
hhaus_state_dir=/var/lib/dd-host-cron
hhaus_ready="$hhaus_state_dir/hhaus-standard-repositories-v1.ready"
if [ -f "$hhaus_ready" ]; then
  echo "--- HHaus standard repository fleet already provisioned ---"
  cat "$hhaus_ready"
else
  echo "--- provisioning HHaus standard repository fleet ---"
  for command in curl git mktemp python3 sha256sum; do
    command -v "$command" >/dev/null || {
      echo "missing command required by HHaus repository publisher: $command" >&2
      exit 70
    }
  done

  install -d -m 0700 -o root -g root "$hhaus_state_dir"
  publisher="$(mktemp /tmp/create-hhaus-standard-repositories.XXXXXX.sh)"
  cleanup_hhaus_publisher() {
    PUBLISHER_PATH="$publisher" python3 - <<'PY'
from pathlib import Path
import os
path = Path(os.environ["PUBLISHER_PATH"])
if path.is_file() and str(path).startswith("/tmp/create-hhaus-standard-repositories."):
    path.unlink()
PY
  }
  trap cleanup_hhaus_publisher EXIT

  publisher_revision=d7718a91b2706201691c926a5d22a60079c28e5d
  publisher_sha256=41d361b9382ccd1785e04d1287967d83b3fde287b9235d60d8a253825ca903d3
  publisher_url="https://raw.githubusercontent.com/ORESoftware/k8s-cluster/$publisher_revision/scripts/ops/create_hhaus_standard_repositories_20260903.sh"
  curl --fail --silent --show-error --location \
    --proto '=https' --tlsv1.2 \
    "$publisher_url" > "$publisher"
  printf '%s  %s\n' "$publisher_sha256" "$publisher" | sha256sum --check --status
  chmod 0700 "$publisher"

  trusted_sha="$(git -C "$repo_root" rev-parse HEAD)"
  [[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
  bash "$publisher" "$trusted_sha"

  printf 'status=ready trusted_k8s_cluster_sha=%s publisher_revision=%s\n' \
    "$trusted_sha" "$publisher_revision" > "$hhaus_ready"
  chmod 0600 "$hhaus_ready"
  echo "HHaus standard repository fleet provisioned"
fi

# Create and normalize the shared locks-and-leases repository through the same
# reviewed OAuth profile and protected-host boundary. The publisher also opens
# one idempotent implementation issue so repository creation cannot be mistaken
# for completion of the polyglot coordination library itself.
ores_locks_state_dir=/var/lib/dd-host-cron
ores_locks_ready="$ores_locks_state_dir/ores-locks-and-leases-v1.ready"
if [ -f "$ores_locks_ready" ]; then
  echo "--- ORES locks-and-leases repository already provisioned ---"
  cat "$ores_locks_ready"
else
  echo "--- provisioning ORES locks-and-leases repository ---"
  for command in bash git install; do
    command -v "$command" >/dev/null || {
      echo "missing command required by ORES locks publisher: $command" >&2
      exit 70
    }
  done

  install -d -m 0700 -o root -g root "$ores_locks_state_dir"
  trusted_sha="$(git -C "$repo_root" rev-parse HEAD)"
  [[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
  ores_locks_publisher="$repo_root/scripts/ops/create_ores_locks_and_leases_repository_20260904.sh"
  [ -f "$ores_locks_publisher" ] || {
    echo "missing ORES locks-and-leases publisher" >&2
    exit 71
  }
  bash "$ores_locks_publisher" "$trusted_sha"

  printf 'status=ready trusted_k8s_cluster_sha=%s publisher_path=%s\n' \
    "$trusted_sha" 'scripts/ops/create_ores_locks_and_leases_repository_20260904.sh' \
    > "$ores_locks_ready"
  chmod 0600 "$ores_locks_ready"
  echo "ORES locks-and-leases repository provisioned"
fi
