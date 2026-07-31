#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

service='remote/deployments/gleamlang-presence-server'
module_dir="$service/src/gleamlang_presence_server"
exporter="$service/scripts/export-openapi.sh"
work_dir='.tmp/gleam-presence-openapi-check'

cleanup_generated_builds() {
  if [ -d "$work_dir" ]; then
    find "$work_dir" -depth -delete
  fi
  while IFS= read -r -d '' generated_dir; do
    find "$generated_dir" -depth -delete
  done < <(find remote/api-sdks -type d \( \
    -name node_modules -o -name dist -o -name target -o \
    -name .dart_tool -o -name build \
  \) -prune -print0)
  find remote/api-sdks -type f \( \
    -name package-lock.json -o -name Cargo.lock -o \
    -name pubspec.lock -o -name manifest.toml \
  \) -delete
}
trap cleanup_generated_builds EXIT

for command in node npm cargo rustc dart gleam python3; do
  command -v "$command" >/dev/null || {
    echo "required command is unavailable: $command" >&2
    exit 1
  }
done

test -x "$exporter"
mkdir -p "$work_dir"

gleam format --check \
  "$module_dir/route_contract.gleam" \
  "$module_dir/openapi_export.gleam" \
  "$module_dir/api_docs.gleam" \
  "$module_dir/http_server.gleam"

bash "$exporter" > "$work_dir/openapi.1.json"
bash "$exporter" > "$work_dir/openapi.2.json"
cmp "$work_dir/openapi.1.json" "$work_dir/openapi.2.json"
cmp "$service/generated/openapi.json" "$work_dir/openapi.1.json"
python3 -m json.tool "$work_dir/openapi.1.json" >/dev/null

python3 - <<'PY'
import json
from pathlib import Path

service = Path('remote/deployments/gleamlang-presence-server')
internal = json.loads((service / 'generated/openapi.json').read_text(encoding='utf-8'))
public = json.loads((service / 'generated/api-docs.json').read_text(encoding='utf-8'))
projected_internal = json.loads(
    (service / 'generated/api-docs.internal.json').read_text(encoding='utf-8')
)

assert internal['openapi'] == '3.1.0'
assert internal['x-dd-contract-scope'] == 'internal'
assert internal['x-dd-native-source-of-truth'] is True
assert projected_internal['openapi'] == '3.1.0'
assert projected_internal['x-dd-contract-scope'] == 'internal'

operations = [
    (path, method, operation)
    for path, item in internal['paths'].items()
    for method, operation in item.items()
]
assert len(operations) == 17, len(operations)
operation_ids = [operation['operationId'] for _, _, operation in operations]
assert len(operation_ids) == len(set(operation_ids)), operation_ids
assert internal['x-dd-operation-count'] == len(operations)
assert internal['x-dd-route-count'] == len(operations)

projected_operations = {
    (path, method): operation
    for path, item in projected_internal['paths'].items()
    for method, operation in item.items()
}
native_operations = {
    (path, method): operation
    for path, method, operation in operations
}
assert set(projected_operations) == set(native_operations)
projected_operation_ids = [
    operation['operationId'] for operation in projected_operations.values()
]
assert len(projected_operation_ids) == len(set(projected_operation_ids))

def resolved_security(document, operation):
    requirements = operation.get('security')
    if requirements is None:
        return None
    schemes = document.get('components', {}).get('securitySchemes', {})
    resolved = []
    for requirement in requirements:
        effective_requirement = []
        for scheme_name, scopes in sorted(requirement.items()):
            scheme = schemes[scheme_name]
            effective_requirement.append(
                (
                    scheme['type'],
                    scheme.get('in'),
                    scheme.get('name'),
                    tuple(scopes),
                )
            )
        resolved.append(effective_requirement)
    return resolved

for key, native in native_operations.items():
    path, method = key
    projected = projected_operations[key]
    projected_handlers = projected['x-dd-handlers']
    assert projected_handlers == [native['operationId']], (key, projected)
    assert projected['x-dd-source-path'] == path, (key, projected)
    assert projected['x-dd-source-paths'] == [path], (key, projected)
    assert projected['x-dd-visibility'] == native['x-dd-visibility'], key
    assert projected['x-dd-auth'] == native['x-dd-auth'], key
    assert projected['x-dd-route-type'] == native['x-dd-route-type'], key
    assert projected['x-dd-implementation'] == 'openapi-code-first', key
    assert resolved_security(projected_internal, projected) == resolved_security(
        internal, native
    ), key
    assert projected['summary'] == native['summary'], key
    assert projected['x-dd-source-files'] == [
        'remote/deployments/gleamlang-presence-server/generated/openapi.json'
    ], key

expected_public = {'/', '/openapi.json', '/api/docs.json', '/api/docs', '/docs/api'}
actual_public = {
    path
    for path, _, operation in operations
    if operation['x-dd-visibility'] == 'public'
}
assert actual_public == expected_public, actual_public
assert set(public['paths']) == expected_public, set(public['paths'])
assert public['x-dd-contract-scope'] == 'public'

required_internal = {
    '/healthz',
    '/nodes',
    '/ws',
    '/conv/{conv_id}/members/{user_id}',
    '/conv/{conv_id}/members',
    '/conv/{conv_id}/broadcast',
    '/user/{user_id}/broadcast',
    '/user/{user_id}/devices/{device_id}/logout',
    '/internal/runtime-config',
    '/internal/update-runtime-config',
    '/internal/runtime-config/reset',
}
assert required_internal <= set(internal['paths'])
assert required_internal.isdisjoint(public['paths'])

for path, method, operation in operations:
    visibility = operation['x-dd-visibility']
    auth = operation['x-dd-auth']
    if visibility == 'public':
        assert operation.get('security') == [], (path, method, operation)
        assert auth == 'public'
    elif auth == 'cluster-network-policy':
        assert 'security' not in operation, (path, method, operation)
    else:
        assert operation.get('security'), (path, method, operation)

    declared = {
        parameter['name']
        for parameter in operation.get('parameters', [])
        if parameter['in'] == 'path'
    }
    templated = {
        part[1:-1]
        for part in path.split('/')
        if part.startswith('{') and part.endswith('}')
    }
    assert declared == templated, (path, declared, templated)

runtime = internal['paths']['/internal/update-runtime-config']['post']
assert runtime['security'] == [{'runtimeConfigServerAuth': []}]
websocket = internal['paths']['/ws']['get']
assert websocket['x-dd-protocol'] == 'websocket'
assert {parameter['name'] for parameter in websocket['parameters']} == {
    'user', 'conv', 'device'
}

public_text = json.dumps(public).lower()
for forbidden in (
    'runtimeconfigserverauth',
    'cluster-network-policy',
    '/internal/',
    '/healthz',
    '/nodes',
    '/ws',
    '/conv/',
    '/user/',
):
    assert forbidden not in public_text, forbidden

for document in (public, projected_internal):
    for item in document['paths'].values():
        for operation in item.values():
            description = operation.get('description', '')
            assert 'runtime Axum router' not in description
            assert 'runtime Fastify router' not in description
            assert 'runtime dispatcher' in description
PY

server="$module_dir/http_server.gleam"
contract="$module_dir/route_contract.gleam"
grep -F 'route_contract.match_route(req.method, request.path_segments(req))' "$server"
grep -F 'case matched.route.id' "$server"
test "$(grep -c '^    route_contract\.' "$server")" -ge 15
if grep -F 'case req.method, request.path_segments(req)' "$server"; then
  echo 'legacy duplicate method/path dispatcher remains' >&2
  exit 1
fi
test "$(grep -c '^    route(' "$contract")" -eq 17

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

node remote/tools/generate-openapi-sdks.mjs \
  --service gleamlang-presence-server \
  --output "$work_dir/generated-client"
cargo check --manifest-path "$work_dir/generated-client/rust/Cargo.toml"
(
  cd "$work_dir/generated-client/typescript"
  npm install --ignore-scripts --no-audit --no-fund --package-lock=false
  npm run build
)
(
  cd "$work_dir/generated-client/dart"
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

cleanup_generated_builds
trap - EXIT

git diff --check
if git grep -nE '^(<<<<<<<|=======|>>>>>>>)' -- . ':!vendor'; then
  echo 'git conflict marker found' >&2
  exit 1
fi
if [[ "${OPENAPI_CHECK_ALLOW_DIRTY:-0}" != '1' ]]; then
  git diff --cached --quiet
  git diff --quiet
  test -z "$(git status --short)"
fi
