#!/usr/bin/env bash
set -euo pipefail

branch='agent/gleam-presence-executable-openapi'
test "${GITHUB_REF_NAME:-$branch}" = "$branch"

service='remote/deployments/gleamlang-presence-server'
module_dir="$service/src/gleamlang_presence_server"
exporter="$service/scripts/export-openapi.sh"
mkdir -p "$service/scripts" "$service/generated"

cat > "$exporter" <<'BASH'
#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
service="$repo_root/remote/deployments/gleamlang-presence-server"
harness="$(mktemp -d "${TMPDIR:-/tmp}/gleam-presence-openapi.XXXXXX")"
cleanup() {
  rm -rf "$harness"
}
trap cleanup EXIT

mkdir -p "$harness/src/gleamlang_presence_server"
cp \
  "$service/src/gleamlang_presence_server/route_contract.gleam" \
  "$harness/src/gleamlang_presence_server/route_contract.gleam"
cp \
  "$service/src/gleamlang_presence_server/openapi_export.gleam" \
  "$harness/src/gleamlang_presence_server/openapi_export.gleam"
cat > "$harness/gleam.toml" <<'TOML'
name = "gleam_presence_contract"
version = "0.1.0"
target = "erlang"
gleam = ">= 1.17.0 and < 2.0.0"

[dependencies]
gleam_stdlib = ">= 0.68.0 and < 2.0.0"
gleam_http = ">= 4.0.0 and < 5.0.0"
gleam_json = ">= 2.0.0 and < 4.0.0"
TOML

cd "$harness"
gleam deps download >&2
gleam check >&2
gleam run -m gleamlang_presence_server/openapi_export
BASH
chmod +x "$exporter"

gleam format \
  "$module_dir/route_contract.gleam" \
  "$module_dir/openapi_export.gleam" \
  "$module_dir/api_docs.gleam" \
  "$module_dir/http_server.gleam"

bash "$exporter" > "$service/generated/openapi.json"
bash "$exporter" > "${RUNNER_TEMP}/gleam-presence-openapi.second.json"
cmp "$service/generated/openapi.json" "${RUNNER_TEMP}/gleam-presence-openapi.second.json"
python3 -m json.tool "$service/generated/openapi.json" >/dev/null

python3 - <<'PY'
import json
from pathlib import Path

manifest_path = Path('remote/api-contracts/manifest.json')
manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
entry = {
    'contract': 'remote/deployments/gleamlang-presence-server/generated/openapi.json',
    'directory': 'remote/deployments/gleamlang-presence-server',
    'docsRoutes': ['/openapi.json', '/api/docs.json', '/api/docs', '/docs/api'],
    'export': [
        'bash',
        'remote/deployments/gleamlang-presence-server/scripts/export-openapi.sh',
    ],
    'implementation': 'gleam-typed-route-registry',
    'language': 'gleam',
    'sdk': {
        'dart': {
            'generator': 'dart',
            'packageName': 'dd_presence_client',
        },
        'rust': {
            'generator': 'rust',
            'packageName': 'dd_presence_client',
        },
        'sourceOfTruthRepository': 'ORESoftware/k8s-libs-and-shared-defs',
        'typescript': {
            'generator': 'typescript-fetch',
            'packageName': '@oresoftware/dd-presence-client',
        },
    },
    'visibility': 'private',
    'publicContract': 'remote/deployments/gleamlang-presence-server/generated/api-docs.json',
    'runtimeContractPolicy': (
        'Unauthenticated standard documentation routes and the service help route serve '
        'only the fail-closed public contract. Health, topology, WebSocket, conversation, '
        'user, and runtime-configuration operations remain in the unserved internal contract.'
    ),
}
existing = manifest['services'].get('gleamlang-presence-server')
if existing is not None and existing != entry:
    raise SystemExit('gleamlang-presence-server manifest entry conflicts with intended native contract')
manifest['services']['gleamlang-presence-server'] = entry
manifest_path.write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')

config_path = Path('remote/config/api-contracts.json')
config = json.loads(config_path.read_text(encoding='utf-8'))
allowlist = config['legacySourceScannerAllowlist']
if allowlist.count('gleamlang-presence-server') > 1:
    raise SystemExit('duplicate gleamlang-presence-server scanner allowlist entries')
config['legacySourceScannerAllowlist'] = [
    name for name in allowlist if name != 'gleamlang-presence-server'
]
config_path.write_text(json.dumps(config, indent=2) + '\n', encoding='utf-8')
PY

node remote/tools/generate-api-docs.mjs
node remote/tools/generate-api-sdks.mjs

node --check remote/tools/check-openapi-contracts.mjs
node --check remote/tools/generate-api-docs.mjs
node --check remote/tools/generate-api-sdks.mjs
node --check remote/tools/generate-openapi-sdks.mjs
node --check remote/tools/validate-openapi-contracts.mjs
node --check remote/tools/validate-api-sdks.mjs
node remote/tools/check-openapi-contracts.mjs --service gleamlang-presence-server
node remote/tools/generate-api-docs.mjs --check --service gleamlang-presence-server
node remote/tools/validate-openapi-contracts.mjs
node remote/tests/check-rest-api-route-parity.mjs
node remote/tools/generate-api-sdks.mjs --check
node remote/tools/validate-api-sdks.mjs

rm -rf .tmp/gleam-presence-sdk
node remote/tools/generate-openapi-sdks.mjs \
  --service gleamlang-presence-server \
  --output .tmp/gleam-presence-sdk
cargo check --manifest-path .tmp/gleam-presence-sdk/rust/Cargo.toml
(
  cd .tmp/gleam-presence-sdk/typescript
  npm install --ignore-scripts --no-audit --no-fund --package-lock=false
  npm run build
)
(
  cd .tmp/gleam-presence-sdk/dart
  dart pub get
  dart analyze
)

for scope in public internal; do
  (
    cd "remote/api-sdks/typescript/$scope"
    npm install --ignore-scripts --no-audit --no-fund --package-lock=false
    npm run build
    npm test
  )
  cargo test --manifest-path "remote/api-sdks/rust/$scope/Cargo.toml"
  (
    cd "remote/api-sdks/dart/$scope"
    dart pub get
    dart analyze
    dart run bin/smoke.dart
  )
  (
    cd "remote/api-sdks/gleam/$scope"
    gleam deps download
    gleam test
  )
done

rm -rf .tmp
find remote/api-sdks -type d \( \
  -name node_modules -o -name dist -o -name target -o \
  -name .dart_tool -o -name build \
\) -prune -exec rm -rf '{}' +
find remote/api-sdks -type f \( \
  -name package-lock.json -o -name Cargo.lock -o \
  -name pubspec.lock -o -name manifest.toml \
\) -delete

rm -f \
  .github/workflows/gleam-presence-contract-diagnostics.yml \
  .github/workflows/gleam-presence-format-materializer.yml \
  scripts/api-contract/finalize-gleam-presence-openapi.sh \
  scripts/api-contract/gleam-presence-finalizer-status.md

git diff --check
if git grep -nE '^(<<<<<<<|=======|>>>>>>>)' -- . ':!vendor'; then
  echo 'git conflict marker found before commit' >&2
  exit 1
fi
test -x "$exporter"
test -s "$service/generated/openapi.json"
test -s "$service/generated/api-docs.json"
test -s "$service/generated/api-docs.internal.json"

git config user.name 'github-actions[bot]'
git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
git add -A
git diff --cached --check
unexpected_bootstrap="$({
  git diff --cached --name-status \
    | awk '$1 != "D" && $2 ~ /(gleam-presence-(contract-diagnostics|format-materializer)|finalize-gleam-presence-openapi|gleam-presence-finalizer-status)/ {print}'
} || true)"
test -z "$unexpected_bootstrap"
git diff --cached --name-status
git commit -m 'feat(api-docs): register native Gleam presence contract and SDKs'
git push origin HEAD:"$branch"
