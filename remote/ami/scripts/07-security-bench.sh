#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# 07-security-bench.sh - Trivy vulnerability scanner and fio benchmark tool
# Runs as: root
# ---------------------------------------------------------------------------
set -euo pipefail

echo "=========================================================="
echo "  07 - Security and Benchmarking Tooling"
echo "=========================================================="

echo "Installing fio..."
dnf install -y fio

echo "Installing Trivy..."
TRIVY_TAG=$(curl -fsSL https://api.github.com/repos/aquasecurity/trivy/releases/latest | jq -r .tag_name)
TRIVY_VERSION="${TRIVY_TAG#v}"
curl -fsSL "https://github.com/aquasecurity/trivy/releases/download/${TRIVY_TAG}/trivy_${TRIVY_VERSION}_Linux-64bit.tar.gz" -o /tmp/trivy.tgz
tar -xzf /tmp/trivy.tgz -C /tmp
install -m 0755 /tmp/trivy /usr/local/bin/trivy

echo ""
echo "--- Security and Benchmark Tool Versions ---"
fio --version
trivy --version

echo "07 - Security and benchmarking tooling installation complete"
