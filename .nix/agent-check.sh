#!/usr/bin/env bash
set -euo pipefail

target="${1:-all}"

export CI="${CI:-1}"
export CHECKPOINT_DISABLE="${CHECKPOINT_DISABLE:-1}"
export NO_COLOR="${NO_COLOR:-1}"
export TF_IN_AUTOMATION="${TF_IN_AUTOMATION:-1}"

if repo_root="$(git rev-parse --show-toplevel 2>/dev/null)"; then
	:
else
	repo_root="$PWD"
fi
cd "$repo_root"

cache_root="${NIX_AGENT_CACHE_ROOT:-$repo_root/.cache/nix-agent}"
mkdir -p "$cache_root"
export XDG_CACHE_HOME="${XDG_CACHE_HOME:-$cache_root/xdg}"
export CARGO_HOME="${CARGO_HOME:-$cache_root/cargo}"
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-$cache_root/gradle}"
export PUB_CACHE="${PUB_CACHE:-$cache_root/dart-pub}"
export npm_config_cache="${npm_config_cache:-$cache_root/npm}"

mkdir -p "$XDG_CACHE_HOME" "$CARGO_HOME" "$GRADLE_USER_HOME" "$PUB_CACHE" "$npm_config_cache"

root_checks() {
	git diff --check
	nixfmt --check flake.nix .nix/dev-shell.nix
	shellcheck .nix/agent-check.sh
	shfmt -d .nix/agent-check.sh
	actionlint \
		.github/workflows/nix.yml \
		.github/workflows/nix-agent-profiles.yml \
		.github/workflows/nix-fleet-audit.yml \
		.github/workflows/repository-catalog.yml
	nix flake check --show-trace
}

catalog_static_checks() {
	ruff check \
		tools/application_catalog.py \
		tools/channel_catalog.py \
		tools/repository_catalog.py \
		tools/test_application_catalog.py \
		tools/test_application_catalog_multisource.py \
		tools/test_channel_catalog.py \
		tools/test_repository_catalog.py
	ruff format --check \
		tools/application_catalog.py \
		tools/channel_catalog.py \
		tools/repository_catalog.py \
		tools/test_application_catalog.py \
		tools/test_application_catalog_multisource.py \
		tools/test_channel_catalog.py \
		tools/test_repository_catalog.py
	nixfmt --check flake.nix .nix/dev-shell.nix
	actionlint .github/workflows/repository-catalog.yml
}

unit_tests() {
	(
		cd tools
		python -m unittest -v \
			test_application_catalog.py \
			test_application_catalog_multisource.py \
			test_channel_catalog.py \
			test_repository_catalog.py
	)
}

validate_fixture() {
	check-jsonschema \
		--schemafile catalog/applications.schema.json \
		catalog/applications.json
	python tools/application_catalog.py validate \
		catalog/applications.json
	python tools/application_catalog.py check \
		catalog/applications.json
	python tools/repository_catalog.py validate \
		catalog/repositories.json \
		--public-safe \
		--repo-root "$PWD"
	python tools/repository_catalog.py validate \
		catalog/fixtures/repositories.v2.json \
		--public-safe \
		--repo-root "$PWD"
	check-jsonschema \
		--schemafile catalog/channels.schema.json \
		catalog/channels.json
	python tools/channel_catalog.py validate \
		catalog/channels.json
	# memebank, hypesiege, and streempilot own Slack channels and Linear
	# projects but are absent from the DEN-598 owners baseline. The gap is
	# allow-listed here so it stays visible until that baseline is recaptured.
	python tools/channel_catalog.py --repo-root "$PWD" check \
		catalog/channels.json \
		--allow-unregistered-owner memebank \
		--allow-unregistered-owner hypesiege \
		--allow-unregistered-owner streempilot
}

build_artifacts() {
	mkdir -p artifacts
	python tools/application_catalog.py report \
		catalog/applications.json \
		--output artifacts/application-catalog.md
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

catalog_ci() {
	catalog_static_checks
	unit_tests
	validate_fixture
	build_artifacts
}

case "$target" in
root)
	root_checks
	;;
unit)
	unit_tests
	;;
static)
	catalog_static_checks
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
ci)
	catalog_ci
	;;
all)
	root_checks
	catalog_ci
	;;
*)
	echo "usage: agent-check [all|root|ci|static|unit|validate|artifacts|collect-public]" >&2
	exit 2
	;;
esac
