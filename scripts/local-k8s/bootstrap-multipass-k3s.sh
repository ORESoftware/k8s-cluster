#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROFILE_PATH="${LOCAL_K8S_PROFILE:-}"

if [[ -z "${PROFILE_PATH}" ]]; then
  printf '%s\n' 'LOCAL_K8S_PROFILE is required' >&2
  exit 2
fi

for command_name in python3 multipass; do
  command -v "${command_name}" >/dev/null 2>&1 || {
    printf 'required command is unavailable: %s\n' "${command_name}" >&2
    exit 2
  }
done

LOCAL_K8S_PROFILE="${PROFILE_PATH}" python3 "${ROOT_DIR}/scripts/local-k8s/verify_profile.py" >/dev/null

profile_value() {
  LOCAL_K8S_PROFILE_PATH="${PROFILE_PATH}" LOCAL_K8S_PROFILE_KEY="$1" python3 - <<'PY'
import json
import os
from pathlib import Path

profile = json.loads(Path(os.environ["LOCAL_K8S_PROFILE_PATH"]).read_text(encoding="utf-8"))
value = profile[os.environ["LOCAL_K8S_PROFILE_KEY"]]
if not isinstance(value, str):
    raise SystemExit("requested profile value is not a string")
print(value)
PY
}

NAME="$(profile_value name)"
RUNTIME="$(profile_value runtime)"
RESOURCE_CLASS="$(profile_value resourceClass)"
K3S_VERSION="$(profile_value runtimeArtifact)"

if [[ "${RUNTIME}" != 'multipass-k3s' ]]; then
  printf 'profile runtime must be multipass-k3s; got %s\n' "${RUNTIME}" >&2
  exit 2
fi

case "${RESOURCE_CLASS}" in
  compact) CPU_COUNT=2; MEMORY_GIB=4; DISK_GIB=30 ;;
  standard) CPU_COUNT=4; MEMORY_GIB=8; DISK_GIB=60 ;;
  large) CPU_COUNT=6; MEMORY_GIB=12; DISK_GIB=100 ;;
  *) printf 'unreachable resource class: %s\n' "${RESOURCE_CLASS}" >&2; exit 2 ;;
esac

VM_NAME="ores-${NAME}"
RUNTIME_DIR="${ROOT_DIR}/tmp/local-k8s/${NAME}"
CLOUD_INIT_PATH="${RUNTIME_DIR}/cloud-init.yaml"
mkdir -p "${RUNTIME_DIR}"

if multipass info "${VM_NAME}" >/dev/null 2>&1; then
  printf 'refusing to reuse existing Multipass VM %s\n' "${VM_NAME}" >&2
  exit 2
fi

cat > "${CLOUD_INIT_PATH}" <<EOF_CLOUD_INIT
#cloud-config
package_update: true
packages:
  - ca-certificates
  - curl
write_files:
  - path: /usr/local/sbin/install-ores-k3s
    owner: root:root
    permissions: '0755'
    content: |
      #!/usr/bin/env bash
      set -Eeuo pipefail
      curl -fsSL https://get.k3s.io -o /var/tmp/install-k3s.sh
      sha256sum /var/tmp/install-k3s.sh > /var/log/ores-k3s-installer.sha256
      INSTALL_K3S_VERSION='${K3S_VERSION}' sh /var/tmp/install-k3s.sh --disable traefik
      systemctl is-enabled k3s
      systemctl is-active k3s
runcmd:
  - [ /usr/local/sbin/install-ores-k3s ]
EOF_CLOUD_INIT

multipass launch 24.04 \
  --name "${VM_NAME}" \
  --cpus "${CPU_COUNT}" \
  --memory "${MEMORY_GIB}G" \
  --disk "${DISK_GIB}G" \
  --cloud-init "${CLOUD_INIT_PATH}"

multipass exec "${VM_NAME}" -- cloud-init status --wait \
  > "${RUNTIME_DIR}/cloud-init-status.txt"
multipass exec "${VM_NAME}" -- sudo k3s kubectl wait \
  --for=condition=Ready nodes --all --timeout=180s
multipass exec "${VM_NAME}" -- sudo k3s kubectl get nodes -o wide \
  > "${RUNTIME_DIR}/nodes.txt"
multipass exec "${VM_NAME}" -- sudo k3s kubectl get --raw='/readyz?verbose' \
  > "${RUNTIME_DIR}/apiserver-readyz.txt"
multipass exec "${VM_NAME}" -- sudo systemctl status k3s --no-pager \
  > "${RUNTIME_DIR}/k3s-systemd-status.txt"

printf 'local K3s VM ready: %s\n' "${VM_NAME}"
printf 'run kubectl: multipass exec %s -- sudo k3s kubectl get pods -A\n' "${VM_NAME}"
printf 'reboot test: multipass restart %s\n' "${VM_NAME}"
printf '%s\n' 'cloud mutations: denied'
