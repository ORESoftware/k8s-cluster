#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# 06-cloud-iac.sh - AWS CLI v2, Terraform, Ansible, and CDK verification
# Runs as: root
# Env:
#   TERRAFORM_VERSION - optional exact Terraform version, otherwise latest
# ---------------------------------------------------------------------------
set -euo pipefail

echo "=========================================================="
echo "  06 - Cloud and IaC Tooling"
echo "=========================================================="

ARCH="amd64"
TERRAFORM_VERSION="${TERRAFORM_VERSION:-}"

echo "Installing AWS CLI v2..."
curl -fsSL https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip -o /tmp/awscliv2.zip
unzip -oq /tmp/awscliv2.zip -d /tmp
/tmp/aws/install --update

echo "Installing Terraform..."
if [[ -z "${TERRAFORM_VERSION}" ]]; then
  TERRAFORM_VERSION=$(curl -fsSL https://checkpoint-api.hashicorp.com/v1/check/terraform | jq -r .current_version)
fi
mkdir -p /tmp/terraform-bin
curl -fsSL "https://releases.hashicorp.com/terraform/${TERRAFORM_VERSION}/terraform_${TERRAFORM_VERSION}_linux_${ARCH}.zip" -o /tmp/terraform.zip
unzip -oq /tmp/terraform.zip -d /tmp/terraform-bin
install -m 0755 /tmp/terraform-bin/terraform /usr/local/bin/terraform

echo "Installing Ansible and Python AWS SDK packages..."
python3 -m pip install --upgrade ansible boto3 botocore

echo "Checking AWS CDK from the Node/nvm phase..."
command -v cdk >/dev/null 2>&1 || echo "Warning: cdk was not found on PATH"

echo ""
echo "--- Cloud and IaC Tool Versions ---"
aws --version
terraform version | head -1
ansible --version | head -1
python3 -c "import boto3; print('boto3:', boto3.__version__)"
cdk --version

echo "06 - Cloud and IaC tooling installation complete"
