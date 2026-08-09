stage=test-fleet
readonly -a TEST_MATRIX=(
  'ores-otel-log-nodejs-test|nodejs|TypeScript/Node.js|npm ci && npm test|root'
  'ores-otel-log-python-test|python|Python|PYTHONPATH=src python3 -m unittest discover -s tests -v|sdk/python'
  'ores-otel-log-go-test|go|Go|go test ./...|sdk/go'
  'ores-otel-log-rust-test|rust|Rust|cargo test --locked|sdk/rust'
  'ores-otel-log-java-test|java|Java|bash test.sh|sdk/java'
  'ores-otel-log-dart-test|dart|Dart|dart pub get && dart run test/conformance.dart|sdk/dart'
  'ores-otel-log-ruby-test|ruby|Ruby|ruby test/next_loggers_test.rb|sdk/ruby'
  'ores-otel-log-gleam-test|gleam|Gleam|gleam test|sdk/gleam'
  'ores-otel-log-erlang-test|erlang|Erlang|bash test.sh|sdk/erlang'
  'ores-otel-log-elixir-test|elixir|Elixir|bash test.sh|sdk/elixir'
  'ores-otel-log-wasm-test|wasm|WebAssembly/Rust|bash test.sh|sdk/wasm'
)

created_or_verified=0
for record in "${TEST_MATRIX[@]}"; do
  IFS='|' read -r repository_name sdk language command workdir <<<"$record"
  full_name="$TEST_ORGANIZATION/$repository_name"
  ensure_public_repository \
    "$TEST_ORGANIZATION" \
    "$repository_name" \
    "$language conformance harness comparing ORESoftware/next-loggers.ts with ores-otel/ores.otel.log."

  if remote_has_main "$full_name"; then
    marker="$(gh api --header "X-GitHub-Api-Version: $API_VERSION" \
      "repos/$full_name/contents/UPSTREAMS.json" --jq .content 2>/dev/null \
      | tr -d '\n' | base64 --decode 2>/dev/null || true)"
    python3 - "$marker" "$BOOTSTRAP_ID" "$sdk" <<'PY'
import json
import sys
record = json.loads(sys.argv[1])
if record.get("bootstrap_id") != sys.argv[2] or record.get("sdk") != sys.argv[3]:
    raise SystemExit("existing test repository does not match the reviewed ORES OTEL fleet")
PY
    printf 'VERIFIED_EXISTING_TEST_REPOSITORY %s sdk=%s\n' "$full_name" "$sdk"
    created_or_verified=$((created_or_verified + 1))
    continue
  fi

  test_root="$work/test-repositories/$repository_name"
  mkdir -p "$test_root"
  cp -a "$canonical_work/sdk/$sdk/." "$test_root/"
  mkdir -p "$test_root/contracts" "$test_root/scripts" "$test_root/.github/workflows"
  cp "$canonical_work/contracts/log-record.schema.json" "$test_root/contracts/"
  cp "$canonical_work/contracts/fixtures/conformance-record.json" "$test_root/contracts/"

  python3 - "$test_root/UPSTREAMS.json" "$BOOTSTRAP_ID" "$full_name" \
    "$sdk" "$language" "$source_main" "$canonical_main" <<'PY'
import json
import sys
from pathlib import Path
path = Path(sys.argv[1])
payload = {
    "schema_version": 1,
    "bootstrap_id": sys.argv[2],
    "repository": sys.argv[3],
    "sdk": sys.argv[4],
    "language": sys.argv[5],
    "legacy": {
        "repository": "ORESoftware/next-loggers.ts",
        "bootstrap_main": sys.argv[6],
    },
    "canonical": {
        "repository": "ores-otel/ores.otel.log",
        "bootstrap_main": sys.argv[7],
    },
    "assertions": [
        "shared log-record JSON Schema is byte-identical",
        "shared conformance fixture is byte-identical",
        "native SDK tests pass against legacy main",
        "native SDK tests pass against canonical main",
    ],
}
path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
PY

  cat > "$test_root/scripts/test-both.sh" <<EOF
#!/usr/bin/env bash
set -Eeuo pipefail

readonly SDK='$sdk'
readonly SDK_WORKDIR='$workdir'
readonly NATIVE_COMMAND='$command'
readonly LEGACY_REPOSITORY='ORESoftware/next-loggers.ts'
readonly CANONICAL_REPOSITORY='ores-otel/ores.otel.log'

work="\$(mktemp -d)"
trap 'rm -rf "\$work"' EXIT

git clone --depth=1 --branch "\${LEGACY_REF:-main}" \
  "https://github.com/\$LEGACY_REPOSITORY.git" "\$work/legacy"
git clone --depth=1 --branch "\${CANONICAL_REF:-main}" \
  "https://github.com/\$CANONICAL_REPOSITORY.git" "\$work/canonical"

diff -u \
  "\$work/legacy/contracts/log-record.schema.json" \
  "\$work/canonical/contracts/log-record.schema.json"
diff -u \
  "\$work/legacy/contracts/fixtures/conformance-record.json" \
  "\$work/canonical/contracts/fixtures/conformance-record.json"

for upstream in legacy canonical; do
  printf '\n=== %s / %s ===\n' "\$upstream" "\$SDK"
  if test "\$SDK_WORKDIR" = root; then
    target="\$work/\$upstream"
  else
    target="\$work/\$upstream/\$SDK_WORKDIR"
  fi
  (cd "\$target" && bash -lc "\$NATIVE_COMMAND")
done
EOF
  chmod 755 "$test_root/scripts/test-both.sh"

  case "$sdk" in
    nodejs)
      setup_steps="$(cat <<'YAML'
      - name: Set up Node.js
        uses: actions/setup-node@v7
        with:
          node-version: '24'
          cache: npm
YAML
)"
      ;;
    python)
      setup_steps="$(cat <<'YAML'
      - name: Set up Python
        uses: actions/setup-python@v7
        with:
          python-version: '3.13'
YAML
)"
      ;;
    go)
      setup_steps="$(cat <<'YAML'
      - name: Set up Go
        uses: actions/setup-go@v7
        with:
          go-version: '1.24.x'
          cache: false
YAML
)"
      ;;
    rust)
      setup_steps="$(cat <<'YAML'
      - name: Set up Rust
        uses: dtolnay/rust-toolchain@stable
YAML
)"
      ;;
    java)
      setup_steps="$(cat <<'YAML'
      - name: Set up Java
        uses: actions/setup-java@v5
        with:
          distribution: temurin
          java-version: '21'
          cache: maven
YAML
)"
      ;;
    dart)
      setup_steps="$(cat <<'YAML'
      - name: Set up Dart
        uses: dart-lang/setup-dart@v1
        with:
          sdk: stable
YAML
)"
      ;;
    ruby)
      setup_steps="$(cat <<'YAML'
      - name: Set up Ruby
        uses: ruby/setup-ruby@v1
        with:
          ruby-version: '3.3'
YAML
)"
      ;;
    gleam)
      setup_steps="$(cat <<'YAML'
      - name: Set up Erlang and Gleam
        uses: erlef/setup-beam@v1
        with:
          otp-version: '28'
          gleam-version: '1.17.0'
YAML
)"
      ;;
    erlang)
      setup_steps="$(cat <<'YAML'
      - name: Set up Erlang and rebar3
        uses: erlef/setup-beam@v1
        with:
          otp-version: '28'
          rebar3-version: '3.27.0'
YAML
)"
      ;;
    elixir)
      setup_steps="$(cat <<'YAML'
      - name: Set up Erlang and Elixir
        uses: erlef/setup-beam@v1
        with:
          otp-version: '28'
          elixir-version: '1.19.0'
YAML
)"
      ;;
    wasm)
      setup_steps="$(cat <<'YAML'
      - name: Set up Rust and WASM target
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown
YAML
)"
      ;;
    *)
      printf 'unsupported SDK in workflow generator: %s\n' "$sdk" >&2
      exit 1
      ;;
  esac

  cat > "$test_root/.github/workflows/conformance.yml" <<EOF
name: Legacy and canonical $language conformance

on:
  workflow_dispatch:
    inputs:
      legacy_ref:
        description: Legacy branch, tag, or commit
        required: true
        default: main
      canonical_ref:
        description: Canonical branch, tag, or commit
        required: true
        default: main

permissions:
  contents: read

concurrency:
  group: \${{ github.workflow }}-\${{ github.ref }}
  cancel-in-progress: true

jobs:
  conformance:
    runs-on: ubuntu-24.04
    timeout-minutes: 30
    steps:
      - name: Check out harness
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
$setup_steps
      - name: Test legacy and canonical SDKs
        env:
          LEGACY_REF: \${{ inputs.legacy_ref }}
          CANONICAL_REF: \${{ inputs.canonical_ref }}
        run: ./scripts/test-both.sh
EOF

  python3 - "$test_root/README.md" "$repository_name" "$language" "$command" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
name, language, command = sys.argv[2:5]
path.write_text(
    f"# {name}\n\n"
    f"Native **{language}** conformance repository for the ORES OTEL logging SDK.\n\n"
    "This harness deliberately tests both upstream identities:\n\n"
    "- legacy compatibility remote: `ORESoftware/next-loggers.ts`\n"
    "- canonical remote: `ores-otel/ores.otel.log`\n\n"
    "It first requires byte-identical shared JSON Schema and conformance fixtures, then runs:\n\n"
    f"`{command}`\n\n"
    "against both repositories. The workflow is manual-only so creating the fleet does not consume "
    "eleven independent CI runs. Run it from **Actions → Legacy and canonical conformance → Run workflow**.\n",
    encoding="utf-8",
)
PY

  if test "$sdk" = nodejs; then
    mkdir -p "$test_root/tests"
    cat > "$test_root/tests/upstreams.test.mjs" <<'EOF'
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

test('declares both canonical and legacy upstreams', async () => {
  const record = JSON.parse(await readFile(new URL('../UPSTREAMS.json', import.meta.url), 'utf8'));
  assert.equal(record.legacy.repository, 'ORESoftware/next-loggers.ts');
  assert.equal(record.canonical.repository, 'ores-otel/ores.otel.log');
});
EOF
  fi

  git -C "$test_root" init -b main
  git -C "$test_root" config user.name 'ORESoftware repository automation'
  git -C "$test_root" config user.email 'bot@oresoftware.dev'
  git -C "$test_root" add -A
  GIT_AUTHOR_DATE='2026-08-08T23:00:00Z' \
  GIT_COMMITTER_DATE='2026-08-08T23:00:00Z' \
    git -C "$test_root" commit -m "test: add $language legacy/canonical conformance harness"
  git -C "$test_root" remote add origin "https://github.com/$full_name.git"
  git -C "$test_root" push -u origin main

  set_topics "$full_name" opentelemetry otel logging conformance testing "${sdk//./-}"
  printf 'CREATED_TEST_REPOSITORY %s sdk=%s language=%s\n' \
    "$full_name" "$sdk" "$language"
  created_or_verified=$((created_or_verified + 1))
done

test "$created_or_verified" -ge 11
printf 'VERIFIED_TEST_FLEET repositories=%s distinct_sdks=11 distinct_languages=10\n' \
  "$created_or_verified"
printf 'bootstrap-stage=%s status=passed\n' "$stage"
