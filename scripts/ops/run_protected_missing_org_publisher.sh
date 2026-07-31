#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:?trusted k8s-cluster SHA required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
publisher_region="${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}"
[[ "$publisher_region" =~ ^[a-z]{2}(-gov)?-[a-z0-9-]+-[0-9]$ ]]
stage=initialization
work="$(mktemp -d /tmp/missing-org-publisher.XXXXXX)"

cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset encoded_pat raw_pat secret_json credential_source ec2_home publisher_region
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  unset GIT_CONFIG_COUNT GIT_CONFIG_KEY_0 GIT_CONFIG_VALUE_0
  rm -rf "$work"
}
report_failure() {
  local rc=$?
  trap - ERR
  printf 'publisher-stage=%s status=failed rc=%s\n' "$stage" "$rc" >&2
  exit "$rc"
}
trap cleanup EXIT
trap report_failure ERR

stage=protected-credential
credential_source=''
GH_TOKEN=''

# Prefer the protected EC2 instance role. This keeps the credential outside the
# GitHub-hosted runner and matches the External Secrets read path.
if command -v aws >/dev/null 2>&1; then
  secret_json="$(
    aws secretsmanager get-secret-value \
      --region "$publisher_region" \
      --secret-id dd/remote-dev/agent-secrets \
      --query SecretString \
      --output text 2>/dev/null || true
  )"
  if test -n "$secret_json"; then
    raw_pat="$(
      printf '%s' "$secret_json" | python3 -c '
import json
import sys
try:
    payload = json.load(sys.stdin)
except (json.JSONDecodeError, OSError):
    raise SystemExit(0)
value = payload.get("GH_PAT")
if isinstance(value, str) and value:
    sys.stdout.write(value)
' 2>/dev/null || true
    )"
    if test -n "$raw_pat" && [[ "$raw_pat" != *$'\n'* && "$raw_pat" != *$'\r'* ]]; then
      GH_TOKEN="$raw_pat"
      credential_source=aws-secrets-manager
    fi
  fi
fi
unset raw_pat secret_json

# Fall back to the reconciled Kubernetes Secret on whichever protected
# kubeconfig is available on the SSM host.
if test -z "$GH_TOKEN" && command -v kubectl >/dev/null 2>&1; then
  for kubeconfig in \
    /etc/kubernetes/admin.conf \
    /root/.kube/config \
    /home/ec2-user/.kube/config
  do
    test -r "$kubeconfig" || continue
    encoded_pat="$(
      KUBECONFIG="$kubeconfig" \
        kubectl -n default get secret dd-agent-secrets \
        -o jsonpath='{.data.GH_PAT}' 2>/dev/null || true
    )"
    test -n "$encoded_pat" || continue
    raw_pat="$(printf '%s' "$encoded_pat" | base64 --decode 2>/dev/null || true)"
    unset encoded_pat
    if test -n "$raw_pat" && [[ "$raw_pat" != *$'\n'* && "$raw_pat" != *$'\r'* ]]; then
      GH_TOKEN="$raw_pat"
      credential_source="kubernetes-secret:${kubeconfig}"
      break
    fi
  done
fi
unset raw_pat encoded_pat

# The protected host also has the authenticated ORESoftware GitHub CLI profile
# requested by the owner. Resolve the account home explicitly and use the CLI
# abstraction rather than parsing hosts.yml, because gh may store the token in
# the operating-system credential store.
if test -z "$GH_TOKEN" && \
   command -v sudo >/dev/null 2>&1 && \
   command -v getent >/dev/null 2>&1; then
  ec2_home="$(getent passwd ec2-user | awk -F: '$1 == "ec2-user" { print $6 }')"
  case "$ec2_home" in
    /*)
      raw_pat="$(
        sudo -u ec2-user -H \
          env \
            -u GH_TOKEN \
            -u GITHUB_TOKEN \
            -u GH_ENTERPRISE_TOKEN \
            -u GITHUB_REPOSITORY_ADMIN_TOKEN \
            -u GH_CONFIG_DIR \
            HOME="$ec2_home" \
            XDG_CONFIG_HOME="$ec2_home/.config" \
            bash -c 'command -v gh >/dev/null 2>&1 && gh auth token --hostname github.com' \
          2>/dev/null || true
      )"
      ;;
    *)
      raw_pat=''
      ;;
  esac
  if test -n "$raw_pat" && \
     [[ "$raw_pat" != *$'\n'* && "$raw_pat" != *$'\r'* && \
        "$raw_pat" != *$'\t'* && "$raw_pat" != *' '* ]]; then
    GH_TOKEN="$raw_pat"
    credential_source=protected-gh-profile
  fi
fi
unset raw_pat ec2_home

if test -z "$GH_TOKEN"; then
  aws_diagnostic=aws-cli-absent
  if command -v aws >/dev/null 2>&1; then
    if aws secretsmanager get-secret-value \
      --region "$publisher_region" \
      --secret-id dd/remote-dev/agent-secrets \
      --query SecretString \
      --output text >/dev/null 2>&1; then
      aws_diagnostic=secret-readable-but-gh-pat-unusable
    else
      aws_diagnostic=secret-unavailable-or-denied
    fi
  fi

  kube_diagnostic=kubectl-absent
  if command -v kubectl >/dev/null 2>&1; then
    kube_diagnostic=no-readable-kubeconfig
    for diagnostic_kubeconfig in \
      /etc/kubernetes/admin.conf \
      /root/.kube/config \
      /home/ec2-user/.kube/config
    do
      test -r "$diagnostic_kubeconfig" || continue
      kube_diagnostic=secret-unavailable-or-empty
      if KUBECONFIG="$diagnostic_kubeconfig" \
        kubectl -n default get secret dd-agent-secrets \
        -o jsonpath='{.data.GH_PAT}' 2>/dev/null | grep -q .; then
        kube_diagnostic=encoded-gh-pat-present-but-unusable
      fi
      break
    done
  fi

  gh_diagnostic=prerequisites-absent
  if command -v sudo >/dev/null 2>&1 && command -v getent >/dev/null 2>&1; then
    diagnostic_home="$(getent passwd ec2-user | awk -F: '$1 == "ec2-user" { print $6 }')"
    case "$diagnostic_home" in
      /*)
        if sudo -u ec2-user -H env \
          -u GH_TOKEN \
          -u GITHUB_TOKEN \
          -u GH_ENTERPRISE_TOKEN \
          -u GITHUB_REPOSITORY_ADMIN_TOKEN \
          -u GH_CONFIG_DIR \
          HOME="$diagnostic_home" \
          XDG_CONFIG_HOME="$diagnostic_home/.config" \
          bash -c 'command -v gh >/dev/null 2>&1'; then
          if sudo -u ec2-user -H env \
            -u GH_TOKEN \
            -u GITHUB_TOKEN \
            -u GH_ENTERPRISE_TOKEN \
            -u GITHUB_REPOSITORY_ADMIN_TOKEN \
            -u GH_CONFIG_DIR \
            HOME="$diagnostic_home" \
            XDG_CONFIG_HOME="$diagnostic_home/.config" \
            bash -c 'gh auth status --hostname github.com >/dev/null 2>&1'; then
            gh_diagnostic=auth-valid-but-token-unavailable
          else
            gh_diagnostic=auth-unavailable
          fi
        else
          gh_diagnostic=gh-cli-absent
        fi
        ;;
      *)
        gh_diagnostic=ec2-home-unresolved
        ;;
    esac
  fi
  unset diagnostic_home diagnostic_kubeconfig

  printf 'publisher-stage=protected-credential status=failed reason=no-readable-protected-credential aws=%s kubernetes=%s gh=%s\n' \
    "$aws_diagnostic" "$kube_diagnostic" "$gh_diagnostic" >&2
  exit 65
fi
export GH_TOKEN
export GITHUB_REPOSITORY_ADMIN_TOKEN="$GH_TOKEN"
printf 'publisher-stage=%s status=passed source=%s\n' "$stage" "$credential_source"

stage=git-credential-transport
git_askpass="$work/git-askpass.sh"
cat > "$git_askpass" <<'ASKPASS_EOF'
#!/usr/bin/env sh
case "${1:-}" in
  *Username*) printf '%s\n' 'x-access-token' ;;
  *Password*) printf '%s\n' "${GH_TOKEN:?GH_TOKEN is required}" ;;
  *) exit 1 ;;
esac
ASKPASS_EOF
chmod 700 "$git_askpass"
export GIT_ASKPASS="$git_askpass"
export GIT_ASKPASS_REQUIRE=force
export GIT_TERMINAL_PROMPT=0
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=credential.helper
export GIT_CONFIG_VALUE_0=
printf 'publisher-stage=%s status=passed\n' "$stage"

stage=github-identity-and-ownership
python3 - <<'PY'
import json
import os
import urllib.error
import urllib.request

token = os.environ['GH_TOKEN']
headers = {
    'Accept': 'application/vnd.github+json',
    'Authorization': f'Bearer {token}',
    'X-GitHub-Api-Version': '2022-11-28',
    'User-Agent': 'protected-missing-org-repository-publisher',
}

def get(path: str) -> dict:
    request = urllib.request.Request('https://api.github.com' + path, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        detail = error.read(4096).decode(errors='replace')
        raise SystemExit(f'GitHub preflight failed for {path}: HTTP {error.code}: {detail}') from error

identity = get('/user')
if identity.get('login') != 'ORESoftware':
    raise SystemExit(f"unexpected publisher identity: {identity.get('login')!r}")

for organization in (
    'hypesiege',
    'StreemPilot',
    'meta-agents-demo',
    'file-tunnel',
    'unreal-unity-poc',
):
    membership = get(f'/user/memberships/orgs/{organization}')
    observed = (membership.get('role'), membership.get('state'))
    if observed != ('admin', 'active'):
        raise SystemExit(f'{organization} owner membership is {observed!r}')
    print(f'{organization} owner membership verified')
PY
printf 'publisher-stage=%s status=passed\n' "$stage"

stage=trusted-publisher-source
git init "$work/k8s-cluster"
git -C "$work/k8s-cluster" remote add origin \
  https://github.com/ORESoftware/k8s-cluster.git
git -C "$work/k8s-cluster" fetch --depth=1 origin "$trusted_sha"
git -C "$work/k8s-cluster" checkout --detach FETCH_HEAD
test "$(git -C "$work/k8s-cluster" rev-parse HEAD)" = "$trusted_sha"
printf 'publisher-stage=%s status=passed sha=%s\n' "$stage" "$trusted_sha"

stage=publisher-static-validation
python3 -m py_compile \
  "$work/k8s-cluster/scripts/ops/publish_missing_org_repositories.py" \
  "$work/k8s-cluster/scripts/ops/publish_missing_org_repositories_current.py" \
  "$work/k8s-cluster/scripts/ops/finalize_missing_org_repositories.py"
printf 'publisher-stage=%s status=passed\n' "$stage"

stage=bounded-repository-publication
cd "$work/k8s-cluster"
python3 scripts/ops/publish_missing_org_repositories_current.py
printf 'publisher-stage=%s status=passed\n' "$stage"

stage=publication-finalization
python3 scripts/ops/finalize_missing_org_repositories.py \
  --json-report "$work/critical-org-publication.json" \
  --markdown-report "$work/critical-org-publication.md" \
  --close-carriers
cat "$work/critical-org-publication.md"
printf 'publisher-stage=%s status=passed\n' "$stage"

stage=complete
printf 'publisher-stage=%s status=success\n' "$stage"
