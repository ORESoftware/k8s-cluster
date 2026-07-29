set -euo pipefail

target="${1:-all}"

static_checks() {
  ruff check tools/repository_catalog.py tools/test_repository_catalog.py
  ruff format --check tools/repository_catalog.py tools/test_repository_catalog.py
  nixfmt --check flake.nix .nix/dev-shell.nix
  actionlint .github/workflows/repository-catalog.yml
}

unit_tests() {
  (
    cd tools
    python -m unittest -v test_repository_catalog.py
  )
}

validate_fixture() {
  python tools/repository_catalog.py validate \
    catalog/repositories.json \
    --public-safe \
    --repo-root "$PWD"
  python tools/repository_catalog.py validate \
    catalog/fixtures/repositories.v2.json \
    --public-safe \
    --repo-root "$PWD"
}

build_artifacts() {
  mkdir -p artifacts
  python tools/repository_catalog.py diff \
    catalog/repositories.json \
    catalog/repositories.json \
    --json-output artifacts/repository-catalog-drift.json \
    --markdown-output artifacts/repository-catalog-drift.md
  python tools/repository_catalog.py dashboard \
    catalog/repositories.json \
    --json-output artifacts/repository-catalog-dashboard.json \
    --markdown-output artifacts/repository-catalog-dashboard.md
  python tools/repository_catalog.py merge-den369 \
    catalog/fixtures/repositories.v2.json \
    catalog/fixtures/den369-report.json \
    --source-path catalog/fixtures/den369-report.json \
    --output artifacts/repository-catalog-with-den369.json
  python tools/repository_catalog.py validate \
    artifacts/repository-catalog-with-den369.json \
    --public-safe \
    --repo-root "$PWD"
}

collect_public() {
  mkdir -p artifacts
  python tools/repository_catalog.py collect \
    --owners catalog/owners.json \
    --visibility public \
    --repo-root "$PWD" \
    --output artifacts/repository-catalog.public.json
  python tools/repository_catalog.py validate \
    artifacts/repository-catalog.public.json \
    --public-safe
}

case "$target" in
  unit)
    unit_tests
    ;;
  static)
    static_checks
    ;;
  validate)
    validate_fixture
    ;;
  artifacts)
    build_artifacts
    ;;
  collect-public)
    collect_public
    ;;
  ci | all)
    static_checks
    unit_tests
    validate_fixture
    build_artifacts
    ;;
  *)
    echo "usage: agent-check [all|ci|static|unit|validate|artifacts|collect-public]" >&2
    exit 2
    ;;
esac
