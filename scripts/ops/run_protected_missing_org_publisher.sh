#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:?trusted k8s-cluster SHA required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
stage=initialization
work="$(mktemp -d /tmp/missing-org-publisher.XXXXXX)"

cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN encoded_pat
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
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
command -v kubectl >/dev/null
test -r /etc/kubernetes/admin.conf
encoded_pat="$(
  KUBECONFIG=/etc/kubernetes/admin.conf \
    kubectl -n default get secret dd-agent-secrets \
    -o jsonpath='{.data.GH_PAT}'
)"
test -n "$encoded_pat"
GH_TOKEN="$(printf '%s' "$encoded_pat" | base64 --decode)"
unset encoded_pat
test -n "$GH_TOKEN"
[[ "$GH_TOKEN" != *$'\n'* && "$GH_TOKEN" != *$'\r'* ]]
export GH_TOKEN
export GITHUB_REPOSITORY_ADMIN_TOKEN="$GH_TOKEN"
printf 'publisher-stage=%s status=passed\n' "$stage"

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
