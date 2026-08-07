#!/usr/bin/env bash
set -euo pipefail

expected_sha="${1:?expected trusted source SHA is required}"
repo_root="${2:?repository root is required}"
json_report="${3:?JSON report path is required}"
markdown_report="${4:?Markdown report path is required}"

if [[ ! "$expected_sha" =~ ^[0-9a-f]{40}$ ]]; then
  printf 'invalid expected source SHA\n' >&2
  exit 64
fi

cd "$repo_root"
actual_sha="$(git rev-parse HEAD)"
if [[ "$actual_sha" != "$expected_sha" ]]; then
  printf 'trusted source mismatch: expected %s, got %s\n' "$expected_sha" "$actual_sha" >&2
  exit 65
fi

origin_url="$(git remote get-url origin)"
case "$origin_url" in
  https://github.com/ORESoftware/k8s-cluster|https://github.com/ORESoftware/k8s-cluster.git|git@github.com:ORESoftware/k8s-cluster.git) ;;
  *)
    printf 'refusing unexpected repository origin: %s\n' "$origin_url" >&2
    exit 66
    ;;
esac

publisher="scripts/ops/publish_zed_pkg_marketing_sites.py"
test_file="tests/ops/test_publish_zed_pkg_marketing_sites.py"
for required in "$publisher" "$test_file"; do
  if [[ ! -f "$required" ]]; then
    printf 'required trusted source is missing: %s\n' "$required" >&2
    exit 67
  fi
done

shopt -s nullglob
publisher_parts=(scripts/ops/publish_zed_pkg_marketing_sites.py.part*)
spec_parts=(scripts/ops/zed_pkg_marketing_sites.json.bz2.b64.part*)
if (( ${#publisher_parts[@]} < 2 || ${#spec_parts[@]} < 2 )); then
  printf 'trusted publisher/specification parts are incomplete\n' >&2
  exit 67
fi
shopt -u nullglob

command -v aws >/dev/null
command -v python3 >/dev/null

secret_id="${AGENT_SECRET_ID:-dd/remote-dev/agent-secrets}"
secret_json="$(aws secretsmanager get-secret-value \
  --secret-id "$secret_id" \
  --query SecretString \
  --output text)"

GH_TOKEN="$(SECRET_JSON="$secret_json" python3 - <<'PY'
import json, os, sys
raw = os.environ.get("SECRET_JSON", "")
try:
    payload = json.loads(raw)
except json.JSONDecodeError:
    sys.exit(2)
for key in ("GH_PAT", "GITHUB_TOKEN", "GH_TOKEN"):
    value = payload.get(key)
    if isinstance(value, str) and value.strip():
        print(value.strip(), end="")
        break
else:
    sys.exit(3)
PY
)"
unset secret_json

if [[ -z "$GH_TOKEN" || "$GH_TOKEN" =~ [[:space:]] ]]; then
  printf 'GitHub publisher credential is unavailable or malformed\n' >&2
  exit 68
fi
export GH_TOKEN
trap 'unset GH_TOKEN' EXIT

python3 -m py_compile "$publisher"
python3 -m unittest "$test_file"
python3 "$publisher" \
  --validate-only \
  --execute \
  --trusted-source-sha "$expected_sha" \
  --workflow-timeout-seconds 1200 \
  --json-report "$json_report" \
  --markdown-report "$markdown_report"

python3 - "$json_report" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
payload = json.loads(path.read_text(encoding="utf-8"))
sites = payload.get("sites", [])
if payload.get("marker") != "zed-pkg-marketing-site-v1":
    raise SystemExit("report marker mismatch")
if len(sites) != 14:
    raise SystemExit(f"expected 14 site results, got {len(sites)}")
failed = [site.get("slug", "<unknown>") for site in sites if not site.get("verified")]
if failed:
    raise SystemExit("unverified marketing sites: " + ", ".join(failed))
PY
