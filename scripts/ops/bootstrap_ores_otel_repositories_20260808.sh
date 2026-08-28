#!/usr/bin/env bash
set -Eeuo pipefail
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$script_dir/bootstrap_ores_otel_repositories_20260808_part1.sh"
source "$script_dir/bootstrap_ores_otel_repositories_20260808_part2.sh"
source "$script_dir/bootstrap_ores_otel_repositories_20260808_part3.sh"
