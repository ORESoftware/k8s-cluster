#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:?trusted k8s-cluster SHA required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
repo_root="$(git rev-parse --show-toplevel)"
test "$(git -C "$repo_root" rev-parse HEAD)" = "$trusted_sha"
work="$(mktemp -d /tmp/elenkos-fleet-publisher.XXXXXX)"
region="${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}"
stage=initialization

cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN raw_secret raw_pat encoded_pat
  rm -rf "$work"
}
fail() {
  rc=$?
  trap - ERR
  printf 'elenkos-publisher stage=%s status=failed rc=%s\n' "$stage" "$rc" >&2
  exit "$rc"
}
trap cleanup EXIT
trap fail ERR

stage=protected-credential
GH_TOKEN=''
credential_source=''
if command -v aws >/dev/null 2>&1; then
  raw_secret="$(aws secretsmanager get-secret-value --region "$region" --secret-id dd/remote-dev/agent-secrets --query SecretString --output text 2>/dev/null || true)"
  if test -n "$raw_secret"; then
    raw_pat="$(printf '%s' "$raw_secret" | python3 -c 'import json,sys
try: d=json.load(sys.stdin)
except Exception: raise SystemExit(0)
v=d.get("GH_PAT")
if isinstance(v,str) and v and not any(c.isspace() for c in v): sys.stdout.write(v)' 2>/dev/null || true)"
    if test -n "$raw_pat"; then GH_TOKEN="$raw_pat"; credential_source=aws-secrets-manager; fi
  fi
fi
unset raw_secret raw_pat

if test -z "$GH_TOKEN" && command -v kubectl >/dev/null 2>&1; then
  for kubeconfig in /etc/kubernetes/admin.conf /root/.kube/config /home/ec2-user/.kube/config; do
    test -r "$kubeconfig" || continue
    encoded_pat="$(KUBECONFIG="$kubeconfig" kubectl -n default get secret dd-agent-secrets -o jsonpath='{.data.GH_PAT}' 2>/dev/null || true)"
    test -n "$encoded_pat" || continue
    raw_pat="$(printf '%s' "$encoded_pat" | base64 --decode 2>/dev/null || true)"
    if test -n "$raw_pat" && [[ "$raw_pat" != *$'\n'* && "$raw_pat" != *$'\r'* && "$raw_pat" != *$'\t'* && "$raw_pat" != *' '* ]]; then
      GH_TOKEN="$raw_pat"; credential_source="kubernetes-secret:${kubeconfig}"; break
    fi
  done
fi
unset raw_pat encoded_pat

if test -z "$GH_TOKEN"; then
  printf 'elenkos-publisher stage=%s status=failed reason=no-readable-protected-credential\n' "$stage" >&2
  exit 65
fi
export GH_TOKEN
printf 'elenkos-publisher stage=%s status=passed source=%s\n' "$stage" "$credential_source"

stage=plan
python3 "$repo_root/scripts/ops/publish_elenkos_fleet.py" --plan > "$work/plan.json"
test "$(jq -r .repository_count "$work/plan.json")" = 21
test "$(jq -r '.production | length' "$work/plan.json")" = 11
test "$(jq -r '.tests | length' "$work/plan.json")" = 10
printf 'elenkos-publisher stage=%s status=passed\n' "$stage"

stage=publish
python3 "$repo_root/scripts/ops/publish_elenkos_fleet.py" --evidence-out "$work/publication.json"
printf 'elenkos-publisher stage=%s status=passed\n' "$stage"

stage=verify
jq -e '
  .repository_count == 21 and
  (.repositories | length == 21) and
  (all(.repositories[]; (.main_sha | test("^[0-9a-f]{40}$")) and (.file_count >= 4))) and
  (([.repositories[].full_name] | unique | length) == 21)
' "$work/publication.json" >/dev/null
while IFS= read -r row; do printf 'published=%s\n' "$row"; done < <(jq -r '.repositories[] | .full_name + "@" + .main_sha' "$work/publication.json")
printf 'elenkos-publisher stage=%s status=passed repositories=21\n' "$stage"

stage=complete
printf 'elenkos-publisher stage=%s status=success\n' "$stage"
