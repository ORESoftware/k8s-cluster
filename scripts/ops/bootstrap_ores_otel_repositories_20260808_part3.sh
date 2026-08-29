stage=organization-profiles
ensure_public_repository 'ores-otel' '.github' \
  'Organization profile and repository map for ORES OTEL.'
ensure_public_repository 'ores-otel-test' '.github' \
  'Organization profile and test-fleet registry for ORES OTEL.'

publish_profile() {
  local owner="$1"
  local kind="$2"
  local full_name="$owner/.github"
  if remote_has_main "$full_name"; then
    printf 'PRESERVE_PROFILE %s\n' "$full_name"
    return
  fi
  local profile_root="$work/profile-$owner"
  mkdir -p "$profile_root/profile"
  git -C "$profile_root" init -b main
  git -C "$profile_root" config user.name 'ORESoftware repository automation'
  git -C "$profile_root" config user.email 'bot@oresoftware.dev'
  if test "$kind" = canonical; then
    cat > "$profile_root/profile/README.md" <<'EOF'
# ORES OTEL

Canonical polyglot structured logging and OpenTelemetry SDKs.

- Canonical implementation: [`ores-otel/ores.otel.log`](https://github.com/ores-otel/ores.otel.log)
- Legacy compatibility remote: [`ORESoftware/next-loggers.ts`](https://github.com/ORESoftware/next-loggers.ts)
- Cross-language validation: [`ores-otel-test`](https://github.com/ores-otel-test)
EOF
  else
    cat > "$profile_root/profile/README.md" <<'EOF'
# ORES OTEL test fleet

Manual, language-native conformance repositories comparing the legacy `ORESoftware/next-loggers.ts` remote with the canonical `ores-otel/ores.otel.log` remote.

The fleet covers Node/TypeScript, Python, Go, Rust, Java, Dart, Ruby, Gleam, Erlang, Elixir, and WASM. Every harness checks the shared JSON Schema and fixture before executing the native SDK tests against both upstreams.
EOF
    python3 - "$profile_root/fleet.json" <<'PY'
import json
import sys
from pathlib import Path
matrix = [
    ("ores-otel-log-nodejs-test", "TypeScript/Node.js"),
    ("ores-otel-log-python-test", "Python"),
    ("ores-otel-log-go-test", "Go"),
    ("ores-otel-log-rust-test", "Rust"),
    ("ores-otel-log-java-test", "Java"),
    ("ores-otel-log-dart-test", "Dart"),
    ("ores-otel-log-ruby-test", "Ruby"),
    ("ores-otel-log-gleam-test", "Gleam"),
    ("ores-otel-log-erlang-test", "Erlang"),
    ("ores-otel-log-elixir-test", "Elixir"),
    ("ores-otel-log-wasm-test", "WebAssembly/Rust"),
]
Path(sys.argv[1]).write_text(
    json.dumps({"schema_version": 1, "repositories": [
        {"name": name, "language": language} for name, language in matrix
    ]}, indent=2) + "\n",
    encoding="utf-8",
)
PY
  fi
  git -C "$profile_root" add -A
  GIT_AUTHOR_DATE='2026-08-08T23:00:00Z' \
  GIT_COMMITTER_DATE='2026-08-08T23:00:00Z' \
    git -C "$profile_root" commit -m 'docs: publish ORES OTEL organization profile'
  git -C "$profile_root" remote add origin "https://github.com/$full_name.git"
  git -C "$profile_root" push -u origin main
  printf 'CREATED_PROFILE %s\n' "$full_name"
}

publish_profile ores-otel canonical
publish_profile ores-otel-test test
printf 'bootstrap-stage=%s status=passed\n' "$stage"

stage=remote-verification
canonical_remote_main="$(gh api --header "X-GitHub-Api-Version: $API_VERSION" \
  "repos/$CANONICAL_REPOSITORY/git/ref/heads/main" --jq .object.sha)"
test "$canonical_remote_main" = "$canonical_main"

verified_tests=0
for record in "${TEST_MATRIX[@]}"; do
  IFS='|' read -r repository_name sdk _language _command _workdir <<<"$record"
  full_name="$TEST_ORGANIZATION/$repository_name"
  remote_main="$(gh api --header "X-GitHub-Api-Version: $API_VERSION" \
    "repos/$full_name/git/ref/heads/main" --jq .object.sha)"
  [[ "$remote_main" =~ ^[0-9a-f]{40}$ ]]
  upstreams="$(gh api --header "X-GitHub-Api-Version: $API_VERSION" \
    "repos/$full_name/contents/UPSTREAMS.json" --jq .content \
    | tr -d '\n' | base64 --decode)"
  python3 - "$upstreams" "$BOOTSTRAP_ID" "$sdk" <<'PY'
import json
import sys
record = json.loads(sys.argv[1])
assert record["bootstrap_id"] == sys.argv[2]
assert record["sdk"] == sys.argv[3]
assert record["legacy"]["repository"] == "ORESoftware/next-loggers.ts"
assert record["canonical"]["repository"] == "ores-otel/ores.otel.log"
PY
  printf 'VERIFIED_TEST_REPOSITORY %s %s\n' "$full_name" "$remote_main"
  verified_tests=$((verified_tests + 1))
done
test "$verified_tests" = 11

printf 'VERIFIED_CANONICAL %s source=%s canonical=%s\n' \
  "$CANONICAL_REPOSITORY" "$source_main" "$canonical_main"
printf 'VERIFIED_REMOTE_FLEET canonical=1 tests=%s profiles=2 total=%s\n' \
  "$verified_tests" "$((verified_tests + 3))"
printf 'bootstrap-stage=%s status=success\n' "$stage"
