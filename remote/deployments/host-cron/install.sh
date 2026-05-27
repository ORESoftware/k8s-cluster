#!/usr/bin/env bash
# Idempotent installer for the host-cron deployment.
# - Copies executables from scripts/ into /usr/local/sbin (mode 0755).
# - Copies systemd units from units/ into /etc/systemd/system.
# - Reloads systemd, then enables and starts every *.timer in units/.
#
# Designed to be invoked from .github/workflows/remote-k8s-maintenance.yml
# during reconcile-runtime, and from an operator shell on the EC2 host.
#
# Must run as root (uses /etc, /usr/local/sbin, systemctl).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
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
