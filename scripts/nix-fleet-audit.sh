#!/usr/bin/env bash
set -euo pipefail

# jq programs are intentionally single-quoted so Bash never expands jq variables.
# shellcheck disable=SC2016

GH_BIN="${GH_BIN:-gh}"
JQ_BIN="${JQ_BIN:-jq}"
output_dir="nix-fleet-audit"
include_archived=false
owners=()

usage() {
	cat <<'EOF'
Usage: bash scripts/nix-fleet-audit.sh --org OWNER [--org OWNER ...] [options]

Read-only audit of GitHub repositories for agent-first Nix, Docker, and OCI posture.

Options:
  --org OWNER            GitHub organization or account to audit; repeatable.
  --org-file PATH        Read one organization/account name per line.
  --output-dir PATH      Output directory (default: nix-fleet-audit).
  --include-archived     Include archived repositories in detailed inspection.
  -h, --help             Show this help.

Environment:
  GH_BIN                  gh-compatible command (default: gh).
  JQ_BIN                  jq-compatible command (default: jq).

Outputs:
  report.json             Machine-readable repository classifications.
  report.md               Human-readable fleet summary.

The command only performs GitHub read operations. It never changes repositories,
branches, pull requests, Actions settings, secrets, packages, or deployments.
EOF
}

require_command() {
	local command_name="$1"
	if ! command -v "$command_name" >/dev/null 2>&1; then
		printf 'required command not found: %s\n' "$command_name" >&2
		exit 127
	fi
}

uri_encode() {
	local value="$1"
	"$JQ_BIN" -nr --arg value "$value" '$value | @uri'
}

repo_file_raw() {
	local repository="$1"
	local ref="$2"
	local path="$3"
	local encoded_ref
	local encoded_path

	encoded_ref="$(uri_encode "$ref")"
	encoded_path="$(uri_encode "$path")"
	"$GH_BIN" api \
		-H 'Accept: application/vnd.github.raw+json' \
		"repos/$repository/contents/$encoded_path?ref=$encoded_ref" 2>/dev/null
}

json_bool() {
	if "$@" >/dev/null 2>&1; then
		printf 'true'
	else
		printf 'false'
	fi
}

has_tree_path() {
	local tree_file="$1"
	local path="$2"
	"$JQ_BIN" -e --arg path "$path" '.tree[]? | select(.path == $path)' "$tree_file" >/dev/null
}

has_tree_prefix() {
	local tree_file="$1"
	local prefix="$2"
	"$JQ_BIN" -e --arg prefix "$prefix" '.tree[]? | select(.path == ($prefix | sub("/$"; "")) or (.path | startswith($prefix)))' "$tree_file" >/dev/null
}

join_missing() {
	local output=""
	local item
	for item in "$@"; do
		if [ -n "$output" ]; then
			output+=", "
		fi
		output+="$item"
	done
	printf '%s' "$output"
}

while (($# > 0)); do
	case "$1" in
	--org)
		if (($# < 2)); then
			printf '%s\n' '--org requires a value' >&2
			exit 64
		fi
		owners+=("$2")
		shift 2
		;;
	--org-file)
		if (($# < 2)); then
			printf '%s\n' '--org-file requires a path' >&2
			exit 64
		fi
		while IFS= read -r owner || [ -n "$owner" ]; do
			owner="${owner%%#*}"
			owner="${owner//[[:space:]]/}"
			if [ -n "$owner" ]; then
				owners+=("$owner")
			fi
		done <"$2"
		shift 2
		;;
	--output-dir)
		if (($# < 2)); then
			printf '%s\n' '--output-dir requires a path' >&2
			exit 64
		fi
		output_dir="$2"
		shift 2
		;;
	--include-archived)
		include_archived=true
		shift
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		printf 'unknown argument: %s\n' "$1" >&2
		usage >&2
		exit 64
		;;
	esac
done

if ((${#owners[@]} == 0)); then
	printf '%s\n' 'at least one --org or --org-file entry is required' >&2
	usage >&2
	exit 64
fi

require_command "$GH_BIN"
require_command "$JQ_BIN"

mkdir -p "$output_dir"
tmp_dir="$(mktemp -d)"
cleanup() {
	if [[ -d "$tmp_dir" ]]; then
		find "$tmp_dir" -depth -delete
	fi
}
trap cleanup EXIT
jsonl="$tmp_dir/repositories.jsonl"
: >"$jsonl"

for owner in "${owners[@]}"; do
	printf 'Auditing %s...\n' "$owner" >&2
	repositories_json="$tmp_dir/${owner//\//_}-repositories.json"
	"$GH_BIN" repo list "$owner" \
		--limit 1000 \
		--json nameWithOwner,isArchived,isFork,visibility,defaultBranchRef,primaryLanguage \
		>"$repositories_json"

	while IFS= read -r repository_json; do
		repository="$("$JQ_BIN" -r '.nameWithOwner' <<<"$repository_json")"
		archived="$("$JQ_BIN" -r '.isArchived' <<<"$repository_json")"
		fork="$("$JQ_BIN" -r '.isFork' <<<"$repository_json")"
		visibility="$("$JQ_BIN" -r '.visibility // "UNKNOWN"' <<<"$repository_json")"
		default_branch="$("$JQ_BIN" -r '.defaultBranchRef.name // ""' <<<"$repository_json")"
		primary_language="$("$JQ_BIN" -r '.primaryLanguage.name // ""' <<<"$repository_json")"

		classification="deferred with reason"
		reason=""
		tree_truncated=false
		has_flake=false
		has_lock=false
		has_dot_nix=false
		has_shell_nix=false
		has_envrc=false
		has_devcontainer=false
		has_compose=false
		has_kubernetes=false
		has_nix_ci=false
		has_agent_ci=false
		has_oci_workflow=false
		has_supply_chain_workflow=false
		dockerfile_count=0
		docker_digest_pinned_all=null
		docker_nonroot_all=null

		if [ "$archived" = "true" ] && [ "$include_archived" != "true" ]; then
			reason="archived repository; detailed inspection skipped"
		elif [ -z "$default_branch" ]; then
			classification="not applicable"
			reason="repository has no default branch"
		else
			tree_file="$tmp_dir/$(printf '%s' "$repository" | tr '/:' '__')-tree.json"
			encoded_branch="$(uri_encode "$default_branch")"
			if ! "$GH_BIN" api "repos/$repository/git/trees/$encoded_branch?recursive=1" >"$tree_file" 2>/dev/null; then
				printf '{"tree":[],"truncated":true}\n' >"$tree_file"
				reason="Git tree could not be read"
			fi

			tree_truncated="$("$JQ_BIN" -r '.truncated // false' "$tree_file")"
			has_flake="$(json_bool has_tree_path "$tree_file" 'flake.nix')"
			has_lock="$(json_bool has_tree_path "$tree_file" 'flake.lock')"
			has_dot_nix="$(json_bool has_tree_prefix "$tree_file" '.nix/')"
			has_shell_nix="$(json_bool has_tree_path "$tree_file" 'shell.nix')"
			has_envrc="$(json_bool has_tree_path "$tree_file" '.envrc')"
			has_devcontainer="$(json_bool has_tree_prefix "$tree_file" '.devcontainer/')"
			has_compose="$(json_bool "$JQ_BIN" -e '.tree[]? | select(.type == "blob") | select(.path | test("(^|/)(docker-compose|compose)([.][^.]+)?[.]ya?ml$"; "i"))' "$tree_file")"
			has_kubernetes="$(json_bool "$JQ_BIN" -e '.tree[]? | select(.path | test("(^|/)(k8s|kubernetes|helm|charts|argocd)(/|$)"; "i"))' "$tree_file")"

			workflow_paths_file="$tmp_dir/$(printf '%s' "$repository" | tr '/:' '__')-workflows.txt"
			"$JQ_BIN" -r '.tree[]? | select(.type == "blob") | select(.path | test("^[.]github/workflows/.*[.]ya?ml$")) | .path' "$tree_file" >"$workflow_paths_file"
			while IFS= read -r workflow_path; do
				[ -n "$workflow_path" ] || continue
				workflow_content="$(repo_file_raw "$repository" "$default_branch" "$workflow_path" || true)"
				if grep -Eiq 'nix[[:space:]]+flake[[:space:]]+check' <<<"$workflow_content"; then
					has_nix_ci=true
				fi
				if grep -Eiq 'nix[[:space:]]+develop[^\n]*agent-check|nix[[:space:]]+run[^\n]*agent-check' <<<"$workflow_content"; then
					has_agent_ci=true
				fi
				if grep -Eiq 'docker/build-push-action|buildah|kaniko|podman|dockerTools[.]build|ghcr[.]io' <<<"$workflow_content"; then
					has_oci_workflow=true
				fi
				if grep -Eiq 'sbom|provenance|cosign|syft|attest|slsa' <<<"$workflow_content"; then
					has_supply_chain_workflow=true
				fi
			done <"$workflow_paths_file"

			docker_paths_file="$tmp_dir/$(printf '%s' "$repository" | tr '/:' '__')-dockerfiles.txt"
			"$JQ_BIN" -r '.tree[]? | select(.type == "blob") | select(.path | test("(^|/)Dockerfile([.][^/]+)?$")) | .path' "$tree_file" >"$docker_paths_file"
			dockerfile_count="$(grep -c . "$docker_paths_file" || true)"
			if ((dockerfile_count > 0)); then
				docker_digest_pinned_all=true
				docker_nonroot_all=true
				while IFS= read -r docker_path; do
					[ -n "$docker_path" ] || continue
					docker_content="$(repo_file_raw "$repository" "$default_branch" "$docker_path" || true)"
					if ! grep -Eiq '^[[:space:]]*FROM[[:space:]]+[^[:space:]]+@sha256:[0-9a-f]{64}([[:space:]]|$)' <<<"$docker_content"; then
						docker_digest_pinned_all=false
					fi
					if ! grep -Eiq '^[[:space:]]*USER[[:space:]]+([^[:space:]]*nonroot|[1-9][0-9]*(:[0-9]+)?|[A-Za-z_][A-Za-z0-9_-]*)[[:space:]]*$' <<<"$docker_content"; then
						docker_nonroot_all=false
					fi
				done <"$docker_paths_file"
			fi

			if [ "$has_flake" = "true" ] && [ "$has_lock" = "true" ] && [ "$has_dot_nix" = "true" ] && [ "$has_nix_ci" = "true" ] && [ "$has_agent_ci" = "true" ]; then
				classification="full flake"
				reason="root flake, lock, .nix implementation, flake CI, and agent command detected"
			elif [ "$has_flake" = "true" ] || [ "$has_shell_nix" = "true" ] || [ "$has_dot_nix" = "true" ]; then
				classification="shell only"
				missing=()
				[ "$has_flake" = "true" ] || missing+=("root flake")
				[ "$has_lock" = "true" ] || missing+=("lock file")
				[ "$has_dot_nix" = "true" ] || missing+=(".nix implementation")
				[ "$has_nix_ci" = "true" ] || missing+=("flake CI")
				[ "$has_agent_ci" = "true" ] || missing+=("agent CI")
				reason="partial Nix adoption; missing $(join_missing "${missing[@]}")"
			else
				classification="deferred with reason"
				reason="no repository-level Nix development contract detected"
			fi
		fi

		"$JQ_BIN" -nc \
			--arg repository "$repository" \
			--arg owner "$owner" \
			--arg default_branch "$default_branch" \
			--arg visibility "$visibility" \
			--arg primary_language "$primary_language" \
			--arg classification "$classification" \
			--arg reason "$reason" \
			--argjson archived "$archived" \
			--argjson fork "$fork" \
			--argjson tree_truncated "$tree_truncated" \
			--argjson has_flake "$has_flake" \
			--argjson has_lock "$has_lock" \
			--argjson has_dot_nix "$has_dot_nix" \
			--argjson has_shell_nix "$has_shell_nix" \
			--argjson has_envrc "$has_envrc" \
			--argjson has_devcontainer "$has_devcontainer" \
			--argjson has_compose "$has_compose" \
			--argjson has_kubernetes "$has_kubernetes" \
			--argjson has_nix_ci "$has_nix_ci" \
			--argjson has_agent_ci "$has_agent_ci" \
			--argjson has_oci_workflow "$has_oci_workflow" \
			--argjson has_supply_chain_workflow "$has_supply_chain_workflow" \
			--argjson dockerfile_count "$dockerfile_count" \
			--argjson docker_digest_pinned_all "$docker_digest_pinned_all" \
			--argjson docker_nonroot_all "$docker_nonroot_all" \
			'{
			  repository: $repository,
			  owner: $owner,
			  default_branch: $default_branch,
			  visibility: $visibility,
			  primary_language: (if $primary_language == "" then null else $primary_language end),
			  archived: $archived,
			  fork: $fork,
			  tree_truncated: $tree_truncated,
			  classification: $classification,
			  reason: $reason,
			  nix: {
			    flake: $has_flake,
			    lock: $has_lock,
			    dot_nix: $has_dot_nix,
			    shell_nix: $has_shell_nix,
			    envrc: $has_envrc,
			    ci_flake_check: $has_nix_ci,
			    ci_agent_check: $has_agent_ci
			  },
			  development: {
			    devcontainer: $has_devcontainer
			  },
			  container: {
			    dockerfile_count: $dockerfile_count,
			    compose: $has_compose,
			    kubernetes: $has_kubernetes,
			    oci_workflow: $has_oci_workflow,
			    digest_pinned_all: $docker_digest_pinned_all,
			    nonroot_all: $docker_nonroot_all,
			    supply_chain_workflow: $has_supply_chain_workflow
			  }
			}' >>"$jsonl"
	done < <("$JQ_BIN" -c 'sort_by(.nameWithOwner)[]' "$repositories_json")
done

"$JQ_BIN" -s 'sort_by(.repository)' "$jsonl" >"$output_dir/report.json"

{
	printf '# Nix, Docker, and OCI fleet audit\n\n'
	printf 'Generated from read-only GitHub API data. Classifications are heuristic and must be reviewed before bulk changes.\n\n'
	printf '## Summary\n\n'
	"$JQ_BIN" -r '
	  group_by(.classification)
	  | map({classification: .[0].classification, count: length})
	  | sort_by(.classification)
	  | .[]
	  | "- **\(.classification):** \(.count)"
	' "$output_dir/report.json"
	printf '\n## Repositories\n\n'
	printf '| Repository | Class | Nix | Agent CI | Dockerfiles | OCI workflow | Reason |\n'
	printf '|---|---|---:|---:|---:|---:|---|\n'
	"$JQ_BIN" -r '
	  .[]
	  | "| `\(.repository)` | \(.classification) | \(if .nix.flake then "flake" else if .nix.shell_nix or .nix.dot_nix then "partial" else "none" end end) | \(if .nix.ci_agent_check then "yes" else "no" end) | \(.container.dockerfile_count) | \(if .container.oci_workflow then "yes" else "no" end) | \(.reason | gsub("\\|"; "\\\\|")) |"
	' "$output_dir/report.json"
} >"$output_dir/report.md"

printf 'Wrote %s and %s\n' "$output_dir/report.json" "$output_dir/report.md" >&2
