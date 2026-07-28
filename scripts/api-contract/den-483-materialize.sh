#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

service='remote/deployments/formal-methods-service-rs'
transient_workflow='.github/workflows/den-483-materialize-formal-openapi.yml'
transient_script='scripts/api-contract/den-483-materialize.sh'
diagnostic='scripts/api-contract/den-483-last-run.md'

bash scripts/api-contract/formal-methods-private-gitlink-shims.sh

mkdir -p "$service/generated"
cargo fmt --manifest-path "$service/Cargo.toml" --all
cargo check --manifest-path "$service/Cargo.toml"
cargo test --manifest-path "$service/Cargo.toml"

cargo run --manifest-path "$service/Cargo.toml" --quiet -- --export-openapi \
  > "$service/generated/openapi.json"
cargo run --manifest-path "$service/Cargo.toml" --quiet -- --export-openapi \
  > "${RUNNER_TEMP}/formal-openapi.second.json"
cmp "$service/generated/openapi.json" "${RUNNER_TEMP}/formal-openapi.second.json"
python3 -m json.tool "$service/generated/openapi.json" >/dev/null

node remote/tools/generate-api-docs.mjs
node remote/tools/generate-api-sdks.mjs

cargo fmt --manifest-path "$service/Cargo.toml" --all -- --check
cargo check --locked --manifest-path "$service/Cargo.toml"
cargo test --locked --manifest-path "$service/Cargo.toml"
node remote/tools/check-openapi-contracts.mjs --service formal-methods-service-rs
node remote/tools/generate-api-docs.mjs --check --service formal-methods-service-rs
node remote/tools/validate-openapi-contracts.mjs
node remote/tools/generate-api-sdks.mjs --check
node remote/tools/validate-api-sdks.mjs

python3 - <<'PY'
import json
from pathlib import Path

directory = Path('remote/deployments/formal-methods-service-rs/generated')
internal = json.loads((directory / 'openapi.json').read_text(encoding='utf-8'))
public = json.loads((directory / 'api-docs.json').read_text(encoding='utf-8'))
expected_public = {'/openapi.json', '/api/docs.json', '/api/docs', '/docs/api'}
assert set(public['paths']) == expected_public, sorted(public['paths'])
assert public['x-dd-contract-scope'] == 'public'
operation = internal['paths']['/webhook/github']['post']
assert operation['operationId'] == 'receiveGitHubFormalMethodsWebhook'
assert operation['security'] == [{'github_webhook_signature': []}]
assert operation['x-dd-max-request-body-bytes'] == 8 * 1024 * 1024
assert set(operation['responses']) == {'200', '202', '400', '401', '413', '500'}
headers = {
    parameter['name']: parameter
    for parameter in operation['parameters']
    if parameter['in'] == 'header'
}
assert set(headers) == {
    'x-hub-signature-256', 'x-github-event', 'x-github-delivery'
}
assert headers['x-hub-signature-256']['required'] is True
assert headers['x-github-event']['required'] is True
assert headers['x-github-delivery']['required'] is False
public_text = json.dumps(public).lower()
for forbidden in (
    '/webhook/github', '/health', '/ready', '/metrics', '/internal/',
    'github_webhook_signature', 'x-hub-signature-256',
    'runtime_config_server_auth', 'github_token_configured',
):
    assert forbidden not in public_text, forbidden
PY

rm -rf .tmp/formal-methods-sdk
node remote/tools/generate-openapi-sdks.mjs \
  --service formal-methods-service-rs \
  --output .tmp/formal-methods-sdk
cargo check --manifest-path .tmp/formal-methods-sdk/rust/Cargo.toml
(
  cd .tmp/formal-methods-sdk/typescript
  npm install --ignore-scripts --no-audit --no-fund --package-lock=false
  npm run build
)
(
  cd .tmp/formal-methods-sdk/dart
  dart pub get
  dart analyze
)

rm -rf .tmp remote/libs
find remote/api-sdks -type d \( \
  -name node_modules -o -name dist -o -name target -o \
  -name .dart_tool -o -name build \
\) -prune -exec rm -rf '{}' +
find remote/api-sdks -type f \( \
  -name package-lock.json -o -name Cargo.lock -o \
  -name pubspec.lock -o -name manifest.toml \
\) -delete
rm -f "$transient_workflow" "$transient_script" "$diagnostic"

git diff --check
if git grep -nE '^(<<<<<<<|=======|>>>>>>>)' -- . ':!vendor'; then
  echo 'git conflict marker found before commit' >&2
  exit 1
fi
test "$(git ls-files --stage remote/libs | awk '{print $1}')" = '160000'
test -s "$service/generated/openapi.json"
test -s "$service/generated/api-docs.json"
test -s "$service/generated/api-docs.internal.json"
test -s "$service/generated/api-docs.metadata.json"

git config user.name 'github-actions[bot]'
git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
git add -A -- . ':(exclude)remote/libs'
git diff --cached --check
test -z "$(git diff --cached --name-only | grep -E 'den-483-(materialize-formal-openapi|materialize\.sh|last-run)' || true)"
git diff --cached --name-status
git commit -m 'feat(DEN-483): materialize formal-methods contracts and SDKs'
git push origin HEAD:agent/den-483-formal-methods-executable-openapi
