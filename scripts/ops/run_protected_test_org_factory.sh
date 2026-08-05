#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:?trusted k8s-cluster SHA required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
publisher_region="${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}"
[[ "$publisher_region" =~ ^[a-z]{2}(-gov)?-[a-z0-9-]+-[0-9]$ ]]
work="$(mktemp -d /tmp/test-org-factory-launcher.XXXXXX)"
stage=initialization

cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset raw_pat encoded_pat secret_json credential_source
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  rm -rf "$work"
}
report_failure() {
  local rc=$?
  trap - ERR
  printf 'test-org-publisher stage=%s status=failed rc=%s\n' "$stage" "$rc" >&2
  exit "$rc"
}
trap cleanup EXIT
trap report_failure ERR

stage=protected-credential
GH_TOKEN=''
credential_source=''

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
import json, sys
try:
    payload = json.load(sys.stdin)
except (json.JSONDecodeError, OSError):
    raise SystemExit(0)
value = payload.get("GH_PAT")
if isinstance(value, str) and value and not any(ch.isspace() for ch in value):
    sys.stdout.write(value)
' 2>/dev/null || true
    )"
    if test -n "$raw_pat"; then
      GH_TOKEN="$raw_pat"
      credential_source=aws-secrets-manager
    fi
  fi
fi
unset raw_pat secret_json

if test -z "$GH_TOKEN" && command -v kubectl >/dev/null 2>&1; then
  for kubeconfig in /etc/kubernetes/admin.conf /root/.kube/config /home/ec2-user/.kube/config; do
    test -r "$kubeconfig" || continue
    encoded_pat="$(
      KUBECONFIG="$kubeconfig" kubectl -n default get secret dd-agent-secrets \
        -o jsonpath='{.data.GH_PAT}' 2>/dev/null || true
    )"
    test -n "$encoded_pat" || continue
    raw_pat="$(printf '%s' "$encoded_pat" | base64 --decode 2>/dev/null || true)"
    if test -n "$raw_pat" && [[ "$raw_pat" != *$'\n'* && "$raw_pat" != *$'\r'* && "$raw_pat" != *' '* ]]; then
      GH_TOKEN="$raw_pat"
      credential_source="kubernetes-secret:${kubeconfig}"
      break
    fi
  done
fi
unset raw_pat encoded_pat

if test -z "$GH_TOKEN"; then
  printf 'test-org-publisher stage=%s status=failed reason=no-protected-github-credential\n' "$stage" >&2
  exit 65
fi
export GH_TOKEN
export GITHUB_REPOSITORY_ADMIN_TOKEN="$GH_TOKEN"
printf 'test-org-publisher stage=%s status=passed source=%s\n' "$stage" "$credential_source"

stage=github-identity-and-ownership
python3 - <<'PY'
import json, os, urllib.error, urllib.request

token = os.environ["GH_TOKEN"]
headers = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {token}",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "protected-test-org-factory-launcher",
}

def get(path: str) -> dict:
    request = urllib.request.Request("https://api.github.com" + path, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        detail = error.read(2048).decode(errors="replace")
        raise SystemExit(f"GitHub preflight failed for {path}: HTTP {error.code}: {detail}") from error

identity = get("/user")
if identity.get("login") != "ORESoftware":
    raise SystemExit(f"unexpected publisher identity: {identity.get('login')!r}")

organizations = (
    "zed-pkg-test",
    "3fa-app-test",
    "declarative-migrations-test",
    "cliptown-test",
    "claritas-viz-test",
    "embedded-alerts-test",
    "evento-globolo-test",
    "fiducia-cloud-test",
    "file-tunnel-test",
    "hypesiege-test",
    "memebank-test",
    "messaging-intel-test",
    "opto-sync-test",
    "quaestor-ledger-test",
    "scintilla-run-test",
    "shared-auth-test",
    "sonus-auris-test",
    "streempilot-test",
)
for organization in organizations:
    membership = get(f"/user/memberships/orgs/{organization}")
    observed = (membership.get("role"), membership.get("state"))
    if observed != ("admin", "active"):
        raise SystemExit(f"{organization} owner membership is {observed!r}")
    print(f"OWNER_VERIFIED {organization}")
PY
printf 'test-org-publisher stage=%s status=passed\n' "$stage"

stage=trusted-source
git init "$work/k8s-cluster" >/dev/null
git -C "$work/k8s-cluster" remote add origin https://github.com/ORESoftware/k8s-cluster.git
git -C "$work/k8s-cluster" fetch --depth=1 origin "$trusted_sha" >/dev/null
git -C "$work/k8s-cluster" checkout --detach FETCH_HEAD >/dev/null
test "$(git -C "$work/k8s-cluster" rev-parse HEAD)" = "$trusted_sha"
printf 'test-org-publisher stage=%s status=passed sha=%s\n' "$stage" "$trusted_sha"

stage=static-validation
payload_dir="$work/k8s-cluster/scripts/ops/test_org_factory"
publisher_encoded="$work/publish_test_org_factory.py.gz.b64"
publisher_script="$work/publish_test_org_factory.py"
source_encoded="$work/source.tar.gz.b64"
source_archive="$work/source.tar.gz"

publisher_parts=("$payload_dir"/publish_test_org_factory.py.gz.b64.part-*)
source_parts=("$payload_dir"/source.tar.gz.b64.part-*)
[[ "${#publisher_parts[@]}" == 2 ]]
[[ "${#source_parts[@]}" == 7 ]]
cat "${publisher_parts[@]}" > "$publisher_encoded"
cat "${source_parts[@]}" > "$source_encoded"
printf '%s  %s\n' '161c0f3aef1756f8970c6d3a720e75f6264d9d929ccf82e9d1d133dd99fa08f0' "$publisher_encoded" | sha256sum --check --strict
printf '%s  %s\n' 'f29ce27911bf17ad07fb1f520a3283811b6310be774d3aa728ab6c712b19cb3f' "$source_encoded" | sha256sum --check --strict
base64 --decode < "$publisher_encoded" | gzip --decompress --stdout > "$publisher_script"
base64 --decode < "$source_encoded" > "$source_archive"
printf '%s  %s\n' '11eef4b3e2452ee022cda36be39a1ccb39fafbaa1190c693a5e092115359ff43' "$publisher_script" | sha256sum --check --strict
printf '%s  %s\n' 'eef10c331cc11f5e927c21cb33481cb7324f3785d73d3dac33f7f3bc74ac7b37' "$source_archive" | sha256sum --check --strict
python3 -m py_compile "$publisher_script"
bash -n "$work/k8s-cluster/scripts/ops/run_protected_test_org_factory.sh"
tar -tzf "$source_archive" >/dev/null
printf 'test-org-publisher stage=%s status=passed\n' "$stage"

stage=bounded-publication
python3 "$publisher_script" \
  --source "$source_archive" \
  --work-root "$work/publisher" \
  --workers 6 \
  --materialize-submodules
printf 'test-org-publisher stage=%s status=passed\n' "$stage"

stage=complete
cat "$work/publisher/summary.json"
printf 'test-org-publisher stage=%s status=success\n' "$stage"
