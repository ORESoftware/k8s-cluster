#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly GH_VERSION='2.94.0'
readonly GH_RELEASE_BASE='https://github.com/cli/cli/releases/download'
readonly GH_AMD64_SHA256='a757f1ba6db18f4de8cbadb244843a5f89bc75b5e7c6fc127d2bd77fbd12ed62'
readonly GH_ARM64_SHA256='705a23b70b0f1b7ba4c302fdcef392ce3edaacfa7ce8e85e4d93d72ea800a538'

fail() {
  printf 'pinned-gh-installer status=failed reason=%s\n' "$*" >&2
  exit 70
}

usage() {
  cat >&2 <<'EOF'
usage:
  install_pinned_github_cli.sh --install-dir PATH
  install_pinned_github_cli.sh --metadata
EOF
  exit 64
}

resolve_platform() {
  case "$(uname -m)" in
    x86_64|amd64)
      GH_ARCH='amd64'
      GH_SHA256="$GH_AMD64_SHA256"
      ;;
    aarch64|arm64)
      GH_ARCH='arm64'
      GH_SHA256="$GH_ARM64_SHA256"
      ;;
    *)
      fail "unsupported-architecture=$(uname -m)"
      ;;
  esac
  GH_ARCHIVE="gh_${GH_VERSION}_linux_${GH_ARCH}.tar.gz"
  GH_URL="${GH_RELEASE_BASE}/v${GH_VERSION}/${GH_ARCHIVE}"
  readonly GH_ARCH GH_SHA256 GH_ARCHIVE GH_URL
}

for command in curl grep head install mktemp sha256sum tar uname; do
  command -v "$command" >/dev/null || fail "missing-command=$command"
done

resolve_platform

case "${1:-}" in
  --metadata)
    [[ $# -eq 1 ]] || usage
    printf 'version=%s\narchitecture=%s\nsha256=%s\nurl=%s\n' \
      "$GH_VERSION" "$GH_ARCH" "$GH_SHA256" "$GH_URL"
    exit 0
    ;;
  --install-dir)
    [[ $# -eq 2 && -n "${2:-}" ]] || usage
    install_dir="$2"
    ;;
  *)
    usage
    ;;
esac

[[ "$install_dir" == /* ]] || fail 'install-dir-must-be-absolute'
install -d -m 0700 "$install_dir"
stage_dir="$(mktemp -d "$install_dir/.stage.XXXXXX")"
cleanup() {
  rm -rf "$stage_dir"
}
trap cleanup EXIT INT TERM

archive="$stage_dir/$GH_ARCHIVE"
printf 'pinned-gh-installer stage=download version=%s arch=%s\n' "$GH_VERSION" "$GH_ARCH" >&2
curl \
  --fail \
  --location \
  --silent \
  --show-error \
  --retry 3 \
  --retry-all-errors \
  --connect-timeout 15 \
  --max-time 180 \
  --proto '=https' \
  --proto-redir '=https' \
  --output "$archive" \
  "$GH_URL"

printf '%s  %s\n' "$GH_SHA256" "$archive" | sha256sum --check --strict >/dev/null
tar \
  --extract \
  --gzip \
  --file "$archive" \
  --directory "$stage_dir" \
  --no-same-owner \
  --no-same-permissions

source_binary="$stage_dir/gh_${GH_VERSION}_linux_${GH_ARCH}/bin/gh"
[[ -x "$source_binary" ]] || fail 'archive-missing-gh-binary'
install -d -m 0700 "$install_dir/bin"
target="$install_dir/bin/gh"
install -m 0755 "$source_binary" "$target"

version_line="$("$target" --version | head -n 1)"
case "$version_line" in
  "gh version ${GH_VERSION} "*) ;;
  *) fail "unexpected-version=$version_line" ;;
esac

printf 'pinned-gh-installer status=passed binary=%s\n' "$target" >&2
printf '%s\n' "$target"
