#!/usr/bin/env bash
# =============================================================================
# sync-images-from-aws.sh — copy pre-built dd images from the AWS node's
# containerd (k8s.io namespace) to ALL Hetzner nodes' containerd, via S3.
# No rebuild: the dd image build is a complex dd-next-1 Cargo-workspace CI, so
# we lift the already-built images straight off the AWS node. Deployments use
# imagePullPolicy: IfNotPresent, so once imported the pods just run.
#
#   ./sync-images-from-aws.sh docker.io/library/dd-remote-web-home:dev [more...]
#
# Mechanism: ctr export on AWS (via SSM) -> S3 -> presigned GET -> ctr import
# on each Hetzner node. Neither side needs the other's credentials.
#
# Env: AWS_INSTANCE, AWS_REGION, BUCKET, SSH_KEY, HETZNER_NODES (space list)
# Prereq once: a transfer bucket whose policy lets the AWS instance role
#   (dd-remote-k8s-role) s3:PutObject — created in this migration.
# =============================================================================
set -euo pipefail

AWS_INSTANCE="${AWS_INSTANCE:-i-0cc2461a55d491af6}"
AWS_REGION="${AWS_REGION:-us-east-1}"
BUCKET="${BUCKET:-dd-img-xfer-710156900967}"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/id_hetzner}"
read -r -a NODES <<<"${HETZNER_NODES:-167.233.100.88 116.203.52.135 204.168.152.145}"
[ $# -ge 1 ] || { echo "usage: $0 <image-ref> [image-ref...]" >&2; exit 1; }

command -v aws >/dev/null || { echo "ERROR: aws CLI required." >&2; exit 1; }

ssm_run() {
  local cid st
  cid=$(aws ssm send-command --instance-ids "$AWS_INSTANCE" --region "$AWS_REGION" \
        --document-name AWS-RunShellScript --parameters commands="[\"$1\"]" \
        --query Command.CommandId --output text) || return 1
  for _ in $(seq 1 240); do
    st=$(aws ssm get-command-invocation --command-id "$cid" --instance-id "$AWS_INSTANCE" \
         --region "$AWS_REGION" --query Status --output text 2>/dev/null || true)
    [ "$st" = Success ] && break
    [ "$st" = Failed ] && { echo "SSM FAILED:" >&2; aws ssm get-command-invocation --command-id "$cid" \
        --instance-id "$AWS_INSTANCE" --region "$AWS_REGION" --query StandardErrorContent --output text >&2; return 1; }
    sleep 3
  done
  aws ssm get-command-invocation --command-id "$cid" --instance-id "$AWS_INSTANCE" \
    --region "$AWS_REGION" --query StandardOutputContent --output text
}

KEY="dd-img-sync-$$.tar"
echo "==> AWS: export $# image(s) -> s3://$BUCKET/$KEY"
ssm_run "sudo ctr -n k8s.io images export /tmp/$KEY $* && aws s3 cp /tmp/$KEY s3://$BUCKET/$KEY --region $AWS_REGION >/dev/null && rm -f /tmp/$KEY && echo exported-ok" \
  | sed 's/^/   /'

URL=$(aws s3 presign "s3://$BUCKET/$KEY" --region "$AWS_REGION" --expires-in 3600)
echo "==> Hetzner: import on ${#NODES[@]} node(s)"
for addr in "${NODES[@]}"; do
  ssh -i "$SSH_KEY" -o StrictHostKeyChecking=accept-new root@"$addr" \
    "curl -fsSL '$URL' -o /tmp/$KEY && ctr -n k8s.io images import /tmp/$KEY >/dev/null && rm -f /tmp/$KEY && echo '   $addr: imported'" </dev/null
done

aws s3 rm "s3://$BUCKET/$KEY" --region "$AWS_REGION" >/dev/null 2>&1 || true
echo "==> done. Restart affected pods to pick up the images:"
echo "    kubectl -n <ns> delete pod -l <selector>   # IfNotPresent => uses the imported image"
