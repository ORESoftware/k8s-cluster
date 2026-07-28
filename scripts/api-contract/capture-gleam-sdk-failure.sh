#!/usr/bin/env bash
set -u

repo_root="${GITHUB_WORKSPACE:-$(pwd)}"
diagnostic="$repo_root/docs/gleam-sdk-diagnostic.txt"
: > "$diagnostic"
status=0

for scope in public internal; do
  package_dir="$repo_root/remote/api-sdks/gleam/$scope"
  {
    echo "===== $scope: gleam deps download ====="
    echo "package_dir=$package_dir"
  } >> "$diagnostic"

  cd "$package_dir" || exit 1
  gleam deps download >> "$diagnostic" 2>&1
  dependency_status=$?
  echo "dependency_exit_code=$dependency_status" >> "$diagnostic"
  if [ "$dependency_status" -ne 0 ]; then
    status=$dependency_status
  fi

  echo "===== $scope: gleam test =====" >> "$diagnostic"
  gleam test >> "$diagnostic" 2>&1
  test_status=$?
  echo "test_exit_code=$test_status" >> "$diagnostic"
  if [ "$test_status" -ne 0 ]; then
    status=$test_status
  fi
done

cd "$repo_root" || exit 1
cat "$diagnostic"
exit "$status"
