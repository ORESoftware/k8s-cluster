#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly BOOTSTRAP_ID='ores-otel-2026-08-08-git-only-v1'
readonly LEGACY_REPOSITORY='ORESoftware/next-loggers.ts'
readonly CANONICAL_REPOSITORY='ores-otel/ores.otel.log'
readonly TEST_ORGANIZATION='ores-otel-test'
readonly EXPECTED_LEGACY_MAIN='05f14768232b770dfc2bbe03f27b388f5a701c74'
readonly EXPECTED_CANONICAL_MAIN='79759db06e2b34d1c270b14784801fee64080453'

: "${GH_TOKEN:?GH_TOKEN is required}"

stage=initialization
work="$(mktemp -d "${RUNNER_TEMP:-/tmp}/ores-otel-git-only.XXXXXX")"

cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  unset GIT_CONFIG_COUNT GIT_CONFIG_KEY_0 GIT_CONFIG_VALUE_0
  rm -rf "$work"
}
report_failure() {
  local rc=$?
  trap - ERR
  printf 'git-only-stage=%s status=failed rc=%s\n' "$stage" "$rc" >&2
  exit "$rc"
}
trap cleanup EXIT INT TERM
trap report_failure ERR

stage=credential-transport
askpass="$work/git-askpass.sh"
cat > "$askpass" <<'ASKPASS'
#!/usr/bin/env sh
case "${1:-}" in
  *Username*) printf '%s\n' x-access-token ;;
  *Password*) printf '%s\n' "${GH_TOKEN:?GH_TOKEN is required}" ;;
  *) exit 1 ;;
esac
ASKPASS
chmod 700 "$askpass"
export GIT_ASKPASS="$askpass"
export GIT_ASKPASS_REQUIRE=force
export GIT_TERMINAL_PROMPT=0
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=credential.helper
export GIT_CONFIG_VALUE_0=
printf 'git-only-stage=%s status=passed\n' "$stage"

remote_main() {
  local repository="$1"
  local sha
  sha="$(
    git ls-remote --exit-code "https://github.com/${repository}.git" refs/heads/main \
      | awk 'NR == 1 {print $1}'
  )"
  [[ "$sha" =~ ^[0-9a-f]{40}$ ]]
  printf '%s' "$sha"
}

stage=upstream-verification
legacy_main="$(remote_main "$LEGACY_REPOSITORY")"
canonical_main="$(remote_main "$CANONICAL_REPOSITORY")"
test "$legacy_main" = "$EXPECTED_LEGACY_MAIN"
test "$canonical_main" = "$EXPECTED_CANONICAL_MAIN"

canonical_work="$work/canonical"
git clone --branch main --single-branch \
  "https://github.com/${CANONICAL_REPOSITORY}.git" "$canonical_work"
test "$(git -C "$canonical_work" rev-parse HEAD)" = "$canonical_main"
git -C "$canonical_work" remote add legacy \
  "https://github.com/${LEGACY_REPOSITORY}.git"
git -C "$canonical_work" fetch --no-tags legacy main
git -C "$canonical_work" merge-base --is-ancestor "$legacy_main" "$canonical_main"

test -s "$canonical_work/contracts/log-record.schema.json"
test -s "$canonical_work/contracts/fixtures/conformance-record.json"
for sdk in nodejs python go rust java dart ruby gleam erlang elixir wasm; do
  test -d "$canonical_work/sdk/$sdk"
  test -s "$canonical_work/contracts/sdk-manifests/$sdk.json"
done
schema_sha256="$(sha256sum "$canonical_work/contracts/log-record.schema.json" | awk '{print $1}')"
fixture_sha256="$(sha256sum "$canonical_work/contracts/fixtures/conformance-record.json" | awk '{print $1}')"
printf 'VERIFIED_UPSTREAM legacy=%s canonical=%s schema=%s fixture=%s\n' \
  "$legacy_main" "$canonical_main" "$schema_sha256" "$fixture_sha256"
printf 'git-only-stage=%s status=passed\n' "$stage"

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

stage=test-fleet-reconciliation
published=0
for record in "${TEST_MATRIX[@]}"; do
  IFS='|' read -r repository_name sdk language native_command upstream_workdir <<<"$record"
  full_name="$TEST_ORGANIZATION/$repository_name"
  repository_url="https://github.com/${full_name}.git"

  before_main="$(remote_main "$full_name")"
  repository_root="$work/test-repositories/$repository_name"
  git clone --branch main --single-branch "$repository_url" "$repository_root"
  test "$(git -C "$repository_root" rev-parse HEAD)" = "$before_main"

  find "$repository_root" -mindepth 1 -maxdepth 1 \
    ! -name '.git' -exec rm -rf -- {} +

  cp -a "$canonical_work/sdk/$sdk/." "$repository_root/"
  mkdir -p \
    "$repository_root/contracts" \
    "$repository_root/scripts" \
    "$repository_root/.github/workflows"
  cp "$canonical_work/contracts/log-record.schema.json" \
    "$repository_root/contracts/log-record.schema.json"
  cp "$canonical_work/contracts/fixtures/conformance-record.json" \
    "$repository_root/contracts/conformance-record.json"
  cp "$canonical_work/contracts/sdk-manifests/$sdk.json" \
    "$repository_root/contracts/sdk-manifest.json"

  python3 - \
    "$repository_root" \
    "$BOOTSTRAP_ID" \
    "$full_name" \
    "$sdk" \
    "$language" \
    "$native_command" \
    "$upstream_workdir" \
    "$legacy_main" \
    "$canonical_main" \
    "$schema_sha256" \
    "$fixture_sha256" <<'PY'
from __future__ import annotations

import json
import stat
import sys
from pathlib import Path

(
    root_text,
    bootstrap_id,
    full_name,
    sdk,
    language,
    native_command,
    upstream_workdir,
    legacy_main,
    canonical_main,
    schema_sha256,
    fixture_sha256,
) = sys.argv[1:]
root = Path(root_text)

record = {
    "schema_version": 1,
    "bootstrap_id": bootstrap_id,
    "repository": full_name,
    "sdk": sdk,
    "language": language,
    "legacy": {
        "repository": "ORESoftware/next-loggers.ts",
        "main": legacy_main,
    },
    "canonical": {
        "repository": "ores-otel/ores.otel.log",
        "main": canonical_main,
    },
    "contracts": {
        "log_record_schema_sha256": schema_sha256,
        "conformance_fixture_sha256": fixture_sha256,
    },
    "native_command": native_command,
    "upstream_workdir": upstream_workdir,
    "assertions": [
        "legacy main is an ancestor of canonical main",
        "shared log-record JSON Schema is byte-identical",
        "shared conformance fixture is byte-identical",
        "native SDK tests pass against legacy main",
        "native SDK tests pass against canonical main",
    ],
}
(root / "UPSTREAMS.json").write_text(
    json.dumps(record, indent=2) + "\n",
    encoding="utf-8",
)

readme = f"""# {full_name.split('/', 1)[1]}

Native **{language}** conformance repository for ORES OTEL.

This repository preserves a language-native copy of the `{sdk}` SDK and verifies both upstream identities:

- legacy compatibility remote: `ORESoftware/next-loggers.ts` at `{legacy_main}`
- canonical remote: `ores-otel/ores.otel.log` at `{canonical_main}`

The harness first compares the shared JSON Schema and conformance fixture byte-for-byte, then runs the native command below against both upstreams:

```sh
{native_command}
```

Run locally with:

```sh
./scripts/test-both.sh
```

The GitHub Actions workflow is intentionally manual-only. It does not consume Actions minutes merely because the fleet repository was created.
"""
(root / "README.md").write_text(readme, encoding="utf-8")

test_both = f"""#!/usr/bin/env bash
set -Eeuo pipefail

readonly SDK={sdk!r}
readonly UPSTREAM_WORKDIR={upstream_workdir!r}
readonly NATIVE_COMMAND={native_command!r}
readonly EXPECTED_LEGACY_MAIN={legacy_main!r}
readonly EXPECTED_CANONICAL_MAIN={canonical_main!r}
readonly EXPECTED_SCHEMA_SHA256={schema_sha256!r}
readonly EXPECTED_FIXTURE_SHA256={fixture_sha256!r}
readonly LEGACY_REPOSITORY='ORESoftware/next-loggers.ts'
readonly CANONICAL_REPOSITORY='ores-otel/ores.otel.log'

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

legacy_ref="${{LEGACY_REF:-$EXPECTED_LEGACY_MAIN}}"
canonical_ref="${{CANONICAL_REF:-$EXPECTED_CANONICAL_MAIN}}"

git clone --no-checkout "https://github.com/$LEGACY_REPOSITORY.git" "$work/legacy"
git -C "$work/legacy" checkout --detach "$legacy_ref"
git clone --no-checkout "https://github.com/$CANONICAL_REPOSITORY.git" "$work/canonical"
git -C "$work/canonical" checkout --detach "$canonical_ref"

test "$(git -C "$work/legacy" rev-parse HEAD)" = "$legacy_ref"
test "$(git -C "$work/canonical" rev-parse HEAD)" = "$canonical_ref"
git -C "$work/canonical" remote add legacy "https://github.com/$LEGACY_REPOSITORY.git"
git -C "$work/canonical" fetch --no-tags legacy "$legacy_ref"
git -C "$work/canonical" merge-base --is-ancestor "$legacy_ref" "$canonical_ref"

test "$(sha256sum "$work/legacy/contracts/log-record.schema.json" | awk '{{print $1}}')" = "$EXPECTED_SCHEMA_SHA256"
test "$(sha256sum "$work/canonical/contracts/log-record.schema.json" | awk '{{print $1}}')" = "$EXPECTED_SCHEMA_SHA256"
test "$(sha256sum "$work/legacy/contracts/fixtures/conformance-record.json" | awk '{{print $1}}')" = "$EXPECTED_FIXTURE_SHA256"
test "$(sha256sum "$work/canonical/contracts/fixtures/conformance-record.json" | awk '{{print $1}}')" = "$EXPECTED_FIXTURE_SHA256"
cmp "$work/legacy/contracts/log-record.schema.json" "$work/canonical/contracts/log-record.schema.json"
cmp "$work/legacy/contracts/fixtures/conformance-record.json" "$work/canonical/contracts/fixtures/conformance-record.json"

for upstream in legacy canonical; do
  printf '\\n=== %s / %s ===\\n' "$upstream" "$SDK"
  if test "$UPSTREAM_WORKDIR" = root; then
    target="$work/$upstream"
  else
    target="$work/$upstream/$UPSTREAM_WORKDIR"
  fi
  (cd "$target" && bash -lc "$NATIVE_COMMAND")
done
"""
script_path = root / "scripts" / "test-both.sh"
script_path.write_text(test_both, encoding="utf-8")
script_path.chmod(script_path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

setups = {
    "nodejs": """      - name: Set up Node.js
        uses: actions/setup-node@v7
        with:
          node-version: '24'
          cache: npm
""",
    "python": """      - name: Set up Python
        uses: actions/setup-python@v7
        with:
          python-version: '3.13'
""",
    "go": """      - name: Set up Go
        uses: actions/setup-go@v7
        with:
          go-version: '1.24.x'
          cache: false
""",
    "rust": """      - name: Set up Rust
        uses: dtolnay/rust-toolchain@stable
""",
    "java": """      - name: Set up Java
        uses: actions/setup-java@v5
        with:
          distribution: temurin
          java-version: '21'
          cache: maven
""",
    "dart": """      - name: Set up Dart
        uses: dart-lang/setup-dart@v1
        with:
          sdk: stable
""",
    "ruby": """      - name: Set up Ruby
        uses: ruby/setup-ruby@v1
        with:
          ruby-version: '3.3'
""",
    "gleam": """      - name: Set up Erlang and Gleam
        uses: erlef/setup-beam@v1
        with:
          otp-version: '28'
          gleam-version: '1.17.0'
""",
    "erlang": """      - name: Set up Erlang and rebar3
        uses: erlef/setup-beam@v1
        with:
          otp-version: '28'
          rebar3-version: '3.27.0'
""",
    "elixir": """      - name: Set up Erlang and Elixir
        uses: erlef/setup-beam@v1
        with:
          otp-version: '28'
          elixir-version: '1.19.0'
""",
    "wasm": """      - name: Set up Rust and WASM target
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown
""",
}
workflow = f"""name: Legacy and canonical {language} conformance

on:
  workflow_dispatch:
    inputs:
      legacy_ref:
        description: Legacy branch, tag, or commit
        required: true
        default: {legacy_main}
      canonical_ref:
        description: Canonical branch, tag, or commit
        required: true
        default: {canonical_main}

permissions:
  contents: read

concurrency:
  group: ${{{{ github.workflow }}}}-${{{{ github.ref }}}}
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
{setups[sdk]}      - name: Test legacy and canonical SDKs
        env:
          LEGACY_REF: ${{{{ inputs.legacy_ref }}}}
          CANONICAL_REF: ${{{{ inputs.canonical_ref }}}}
        run: ./scripts/test-both.sh
"""
(root / ".github" / "workflows" / "conformance.yml").write_text(
    workflow,
    encoding="utf-8",
)

if sdk == "nodejs":
    package_path = root / "package.json"
    package = json.loads(package_path.read_text(encoding="utf-8"))
    package["name"] = "@ores-otel-test/ores-otel-log-nodejs-test"
    package["private"] = True
    package["scripts"] = {
        "test": "node --test tests/upstreams.test.mjs",
        "test:upstreams": "./scripts/test-both.sh",
    }
    package.pop("publishConfig", None)
    package_path.write_text(json.dumps(package, indent=2) + "\n", encoding="utf-8")
    tests = root / "tests"
    tests.mkdir(parents=True, exist_ok=True)
    (tests / "upstreams.test.mjs").write_text(
        """import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

test('declares canonical and legacy upstreams', async () => {
  const record = JSON.parse(
    await readFile(new URL('../UPSTREAMS.json', import.meta.url), 'utf8'),
  );
  assert.equal(record.legacy.repository, 'ORESoftware/next-loggers.ts');
  assert.equal(record.canonical.repository, 'ores-otel/ores.otel.log');
  assert.match(record.legacy.main, /^[0-9a-f]{40}$/);
  assert.match(record.canonical.main, /^[0-9a-f]{40}$/);
});
""",
        encoding="utf-8",
    )
PY

  git -C "$repository_root" config user.name 'ORESoftware repository automation'
  git -C "$repository_root" config user.email 'bot@oresoftware.dev'
  git -C "$repository_root" add -A

  if git -C "$repository_root" diff --cached --quiet; then
    after_main="$before_main"
    printf 'UNCHANGED_TEST_REPOSITORY %s main=%s\n' "$full_name" "$after_main"
  else
    git -C "$repository_root" commit \
      -m "test: install $language legacy/canonical conformance harness"
    after_main="$(git -C "$repository_root" rev-parse HEAD)"
    git -C "$repository_root" push origin HEAD:refs/heads/main
    test "$(remote_main "$full_name")" = "$after_main"
    printf 'UPDATED_TEST_REPOSITORY %s sdk=%s language=%s before=%s after=%s\n' \
      "$full_name" "$sdk" "$language" "$before_main" "$after_main"
  fi

  published=$((published + 1))
done

test "$published" -eq 11
printf 'git-only-stage=%s status=passed repositories=%s\n' "$stage" "$published"

stage=organization-profile
profile_name="$TEST_ORGANIZATION/.github"
profile_root="$work/organization-profile"
profile_before="$(remote_main "$profile_name")"
git clone --branch main --single-branch \
  "https://github.com/${profile_name}.git" "$profile_root"
test "$(git -C "$profile_root" rev-parse HEAD)" = "$profile_before"
mkdir -p "$profile_root/profile"

python3 - "$profile_root" "$canonical_main" "$legacy_main" <<'PY'
from __future__ import annotations

import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
canonical_main = sys.argv[2]
legacy_main = sys.argv[3]
matrix = [
    ("ores-otel-log-nodejs-test", "TypeScript/Node.js", "nodejs"),
    ("ores-otel-log-python-test", "Python", "python"),
    ("ores-otel-log-go-test", "Go", "go"),
    ("ores-otel-log-rust-test", "Rust", "rust"),
    ("ores-otel-log-java-test", "Java", "java"),
    ("ores-otel-log-dart-test", "Dart", "dart"),
    ("ores-otel-log-ruby-test", "Ruby", "ruby"),
    ("ores-otel-log-gleam-test", "Gleam", "gleam"),
    ("ores-otel-log-erlang-test", "Erlang", "erlang"),
    ("ores-otel-log-elixir-test", "Elixir", "elixir"),
    ("ores-otel-log-wasm-test", "WebAssembly/Rust", "wasm"),
]
lines = [
    "# ORES OTEL test fleet",
    "",
    "Language-native conformance repositories comparing the preserved legacy remote "
    "with the canonical ORES OTEL repository.",
    "",
    f"- Legacy: `ORESoftware/next-loggers.ts` at `{legacy_main}`",
    f"- Canonical: `ores-otel/ores.otel.log` at `{canonical_main}`",
    "",
    "| Repository | Language/target |",
    "| --- | --- |",
]
for name, language, _sdk in matrix:
    lines.append(f"| [`{name}`](https://github.com/ores-otel-test/{name}) | {language} |")
lines.extend(
    [
        "",
        "Each repository contains the native SDK, the shared JSON Schema and fixture, "
        "an `UPSTREAMS.json` provenance manifest, and a manual conformance workflow.",
        "",
    ]
)
(root / "profile" / "README.md").write_text("\n".join(lines), encoding="utf-8")
(root / "fleet.json").write_text(
    json.dumps(
        {
            "schema_version": 1,
            "legacy": {
                "repository": "ORESoftware/next-loggers.ts",
                "main": legacy_main,
            },
            "canonical": {
                "repository": "ores-otel/ores.otel.log",
                "main": canonical_main,
            },
            "repositories": [
                {"name": name, "language": language, "sdk": sdk}
                for name, language, sdk in matrix
            ],
        },
        indent=2,
    )
    + "\n",
    encoding="utf-8",
)
PY

git -C "$profile_root" config user.name 'ORESoftware repository automation'
git -C "$profile_root" config user.email 'bot@oresoftware.dev'
git -C "$profile_root" add profile/README.md fleet.json
if git -C "$profile_root" diff --cached --quiet; then
  profile_after="$profile_before"
  printf 'UNCHANGED_PROFILE %s main=%s\n' "$profile_name" "$profile_after"
else
  git -C "$profile_root" commit -m 'docs: publish ORES OTEL test fleet registry'
  profile_after="$(git -C "$profile_root" rev-parse HEAD)"
  git -C "$profile_root" push origin HEAD:refs/heads/main
  test "$(remote_main "$profile_name")" = "$profile_after"
  printf 'UPDATED_PROFILE %s before=%s after=%s\n' \
    "$profile_name" "$profile_before" "$profile_after"
fi
printf 'git-only-stage=%s status=passed\n' "$stage"

stage=independent-remote-verification
verified=0
for record in "${TEST_MATRIX[@]}"; do
  IFS='|' read -r repository_name sdk language native_command upstream_workdir <<<"$record"
  full_name="$TEST_ORGANIZATION/$repository_name"
  verify_root="$work/verification/$repository_name"
  git clone --depth=1 --branch main --single-branch \
    "https://github.com/${full_name}.git" "$verify_root"
  remote_sha="$(remote_main "$full_name")"
  test "$(git -C "$verify_root" rev-parse HEAD)" = "$remote_sha"
  test -x "$verify_root/scripts/test-both.sh"
  test -s "$verify_root/.github/workflows/conformance.yml"
  test -s "$verify_root/contracts/log-record.schema.json"
  test -s "$verify_root/contracts/conformance-record.json"
  test -s "$verify_root/contracts/sdk-manifest.json"
  test "$(sha256sum "$verify_root/contracts/log-record.schema.json" | awk '{print $1}')" = "$schema_sha256"
  test "$(sha256sum "$verify_root/contracts/conformance-record.json" | awk '{print $1}')" = "$fixture_sha256"

  python3 - \
    "$verify_root/UPSTREAMS.json" \
    "$BOOTSTRAP_ID" \
    "$full_name" \
    "$sdk" \
    "$language" \
    "$legacy_main" \
    "$canonical_main" \
    "$schema_sha256" \
    "$fixture_sha256" <<'PY'
import json
import sys
from pathlib import Path

(
    path,
    bootstrap_id,
    full_name,
    sdk,
    language,
    legacy_main,
    canonical_main,
    schema_sha256,
    fixture_sha256,
) = sys.argv[1:]
record = json.loads(Path(path).read_text(encoding="utf-8"))
assert record["schema_version"] == 1
assert record["bootstrap_id"] == bootstrap_id
assert record["repository"] == full_name
assert record["sdk"] == sdk
assert record["language"] == language
assert record["legacy"] == {
    "repository": "ORESoftware/next-loggers.ts",
    "main": legacy_main,
}
assert record["canonical"] == {
    "repository": "ores-otel/ores.otel.log",
    "main": canonical_main,
}
assert record["contracts"] == {
    "log_record_schema_sha256": schema_sha256,
    "conformance_fixture_sha256": fixture_sha256,
}
PY
  printf 'VERIFIED_TEST_REPOSITORY %s main=%s sdk=%s language=%s\n' \
    "$full_name" "$remote_sha" "$sdk" "$language"
  verified=$((verified + 1))
done

test "$verified" -eq 11
verify_profile="$work/verification/profile"
git clone --depth=1 --branch main --single-branch \
  "https://github.com/${profile_name}.git" "$verify_profile"
test -s "$verify_profile/fleet.json"
python3 - "$verify_profile/fleet.json" "$legacy_main" "$canonical_main" <<'PY'
import json
import sys
from pathlib import Path

record = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert record["schema_version"] == 1
assert record["legacy"]["main"] == sys.argv[2]
assert record["canonical"]["main"] == sys.argv[3]
assert len(record["repositories"]) == 11
assert len({item["name"] for item in record["repositories"]}) == 11
assert len({item["sdk"] for item in record["repositories"]}) == 11
PY

printf 'VERIFIED_GIT_ONLY_FLEET canonical=%s legacy=%s tests=%s profile=%s\n' \
  "$canonical_main" "$legacy_main" "$verified" "$(git -C "$verify_profile" rev-parse HEAD)"
printf 'git-only-stage=%s status=success\n' "$stage"
