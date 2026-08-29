#!/usr/bin/env bash
set -euo pipefail

# jq assertions are intentionally single-quoted so Bash never expands jq fields.
# shellcheck disable=SC2016

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
tmp_dir="$(mktemp -d)"
cleanup() {
	if [[ -d "$tmp_dir" ]]; then
		find "$tmp_dir" -depth -delete
	fi
}
trap cleanup EXIT

mock_gh="$tmp_dir/gh"
cat >"$mock_gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

if [ "${1:-}" = "repo" ] && [ "${2:-}" = "list" ] && [ "${3:-}" = "test-org" ]; then
	cat <<'JSON'
[
  {
    "nameWithOwner": "test-org/full",
    "isArchived": false,
    "isFork": false,
    "visibility": "PUBLIC",
    "defaultBranchRef": {"name": "main"},
    "primaryLanguage": {"name": "Rust"}
  },
  {
    "nameWithOwner": "test-org/partial",
    "isArchived": false,
    "isFork": false,
    "visibility": "PRIVATE",
    "defaultBranchRef": {"name": "trunk"},
    "primaryLanguage": {"name": "Shell"}
  },
  {
    "nameWithOwner": "test-org/archived",
    "isArchived": true,
    "isFork": false,
    "visibility": "PUBLIC",
    "defaultBranchRef": {"name": "main"},
    "primaryLanguage": null
  }
]
JSON
	exit 0
fi

if [ "${1:-}" = "api" ]; then
	endpoint="${!#}"
	case "$endpoint" in
	"repos/test-org/full/git/trees/main?recursive=1")
		cat <<'JSON'
{
  "truncated": false,
  "tree": [
    {"path": "flake.nix", "type": "blob"},
    {"path": "flake.lock", "type": "blob"},
    {"path": ".nix", "type": "tree"},
    {"path": ".nix/dev-shell.nix", "type": "blob"},
    {"path": ".envrc", "type": "blob"},
    {"path": ".github/workflows/nix.yml", "type": "blob"},
    {"path": "Dockerfile", "type": "blob"},
    {"path": "k8s/deployment.yaml", "type": "blob"}
  ]
}
JSON
		;;
	"repos/test-org/partial/git/trees/trunk?recursive=1")
		cat <<'JSON'
{
  "truncated": false,
  "tree": [
    {"path": "shell.nix", "type": "blob"},
    {"path": "compose.yml", "type": "blob"}
  ]
}
JSON
		;;
	*"repos/test-org/full/contents/.github%2Fworkflows%2Fnix.yml?ref=main")
		cat <<'YAML'
name: nix
jobs:
  check:
    steps:
      - run: nix flake check --show-trace
      - run: nix develop -c agent-check
      - uses: docker/build-push-action@0123456789abcdef
        with:
          provenance: mode=max
          sbom: true
YAML
		;;
	*"repos/test-org/full/contents/Dockerfile?ref=main")
		cat <<'DOCKER'
FROM rust:1.95@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa AS build
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
USER 65532:65532
DOCKER
		;;
	*)
		printf 'unexpected mock gh endpoint: %s\n' "$endpoint" >&2
		exit 1
		;;
	esac
	exit 0
fi

printf 'unexpected mock gh arguments: %s\n' "$*" >&2
exit 1
EOF
chmod +x "$mock_gh"

output_dir="$tmp_dir/output"
GH_BIN="$mock_gh" bash "$repo_root/scripts/nix-fleet-audit.sh" \
	--org test-org \
	--output-dir "$output_dir"

jq -e 'length == 3' "$output_dir/report.json" >/dev/null
jq -e '.[] | select(.repository == "test-org/full") | .classification == "full flake"' "$output_dir/report.json" >/dev/null
jq -e '.[] | select(.repository == "test-org/full") | .nix.ci_agent_check == true' "$output_dir/report.json" >/dev/null
jq -e '.[] | select(.repository == "test-org/full") | .container.dockerfile_count == 1' "$output_dir/report.json" >/dev/null
jq -e '.[] | select(.repository == "test-org/full") | .container.digest_pinned_all == true' "$output_dir/report.json" >/dev/null
jq -e '.[] | select(.repository == "test-org/full") | .container.nonroot_all == true' "$output_dir/report.json" >/dev/null
jq -e '.[] | select(.repository == "test-org/full") | .container.supply_chain_workflow == true' "$output_dir/report.json" >/dev/null
jq -e '.[] | select(.repository == "test-org/partial") | .classification == "shell only"' "$output_dir/report.json" >/dev/null
jq -e '.[] | select(.repository == "test-org/archived") | .classification == "deferred with reason"' "$output_dir/report.json" >/dev/null
grep -Fq '| `test-org/full` | full flake |' "$output_dir/report.md"

printf '%s\n' 'nix-fleet-audit fixture test passed'
