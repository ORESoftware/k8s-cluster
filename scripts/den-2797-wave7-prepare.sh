#!/usr/bin/env bash
set -Eeuo pipefail
export CARGO_TERM_COLOR=always
work=/tmp/den-2797-wave7
rm -rf "$work"
mkdir -p "$work"

checkout_exact() {
  local repository="$1" sha="$2" destination="$3"
  git clone --quiet --filter=blob:none "https://github.com/${repository}.git" "$destination"
  git -C "$destination" checkout --quiet --detach "$sha"
  test "$(git -C "$destination" rev-parse HEAD)" = "$sha"
}

verify_bundle() {
  local bundle="$1" expected_sha="$2" expected_head="$3" label="$4"
  test "$(sha256sum "$bundle" | awk '{print $1}')" = "$expected_sha"
  local verify_repo="$work/verify-${label}.git"
  git init --quiet --bare "$verify_repo"
  git -C "$verify_repo" bundle verify "$bundle"
  test "$(git bundle list-heads "$bundle" refs/heads/main | awk '{print $1}')" = "$expected_head"
}

init_reconstructed_repo() {
  local directory="$1" message_file="$2" fixed_date="$3"
  git -C "$directory" init --quiet --initial-branch=main
  git -C "$directory" config user.name "ORESoftware automation"
  git -C "$directory" config user.email "11139560+ORESoftware@users.noreply.github.com"
  git -C "$directory" add --all
  GIT_AUTHOR_DATE="$fixed_date" GIT_COMMITTER_DATE="$fixed_date" \
    git -C "$directory" commit --quiet --file "$message_file"
}

rust_toolchain=1.97.1
rustup toolchain install "$rust_toolchain" --profile minimal --component rustfmt --component clippy

checkout_exact led-dynamo/.github "$LED_SOURCE_SHA" "$work/led-source"
base64 --decode \
  "$work/led-source/.artifacts/repository-recovery-wave7/leddy-sync.bundle.b64" \
  > "$work/leddy-sync.bundle"
base64 --decode \
  "$work/led-source/.artifacts/repository-recovery-wave7/leddy-mcp-server.rs.bundle.b64" \
  > "$work/leddy-mcp-server.rs.bundle"
verify_bundle "$work/leddy-sync.bundle" "$LEDDY_SYNC_BUNDLE_SHA256" "$LEDDY_SYNC_HEAD" leddy-sync
verify_bundle "$work/leddy-mcp-server.rs.bundle" "$LEDDY_MCP_BUNDLE_SHA256" "$LEDDY_MCP_HEAD" leddy-mcp
git clone --quiet "$work/leddy-sync.bundle" "$work/leddy-sync"
git clone --quiet "$work/leddy-mcp-server.rs.bundle" "$work/leddy-mcp-server.rs"
test "$(git -C "$work/leddy-sync" rev-parse HEAD)" = "$LEDDY_SYNC_HEAD"
test "$(git -C "$work/leddy-mcp-server.rs" rev-parse HEAD)" = "$LEDDY_MCP_HEAD"
cargo +"$rust_toolchain" fmt --manifest-path "$work/leddy-sync/Cargo.toml" --all -- --check
cargo +"$rust_toolchain" clippy --manifest-path "$work/leddy-sync/Cargo.toml" --all-targets --all-features -- -D warnings
cargo +"$rust_toolchain" test --manifest-path "$work/leddy-sync/Cargo.toml" --all-targets --all-features

# Preserve the recovered MCP commit as immutable ancestry, then add the exact
# formatting-only repair required by the pinned Rust 1.97.1/rustfmt 1.9.0 CI
# contract. Fail closed unless rustfmt produces only the reviewed one-file diff.
git -C "$work/leddy-mcp-server.rs" config user.name "ORESoftware automation"
git -C "$work/leddy-mcp-server.rs" config user.email "11139560+ORESoftware@users.noreply.github.com"
cargo +"$rust_toolchain" fmt --manifest-path "$work/leddy-mcp-server.rs/Cargo.toml" --all
test "$(git -C "$work/leddy-mcp-server.rs" diff --name-only)" = "src/main.rs"
git -C "$work/leddy-mcp-server.rs" diff --check
git -C "$work/leddy-mcp-server.rs" diff -- src/main.rs > "$work/leddy-mcp-rustfmt.patch"
test "$(sha256sum "$work/leddy-mcp-rustfmt.patch" | awk '{print $1}')" = \
  b51ffee7cdaeea73461f9740e058b0f2d6e949111f544d25c9a80724e543a0b4
git -C "$work/leddy-mcp-server.rs" add src/main.rs
GIT_AUTHOR_DATE=2026-08-09T20:03:00Z GIT_COMMITTER_DATE=2026-08-09T20:03:00Z \
  git -C "$work/leddy-mcp-server.rs" commit --quiet \
    -m "style: normalize MCP source with Rust 1.97.1 rustfmt" \
    -m "Preserve recovered commit 6b8df986bcdd37a3aafdd1a97e1703c8db0379f6 as the parent; this formatting-only commit makes the stable Rust 1.97.1 CI contract reproducible without rewriting recovered history."
LEDDY_MCP_EXPECTED_SHA="$(git -C "$work/leddy-mcp-server.rs" rev-parse HEAD)"
test "$LEDDY_MCP_EXPECTED_SHA" = 45253d520208a1358bd150290eddd5a6eecc5f5e
test "$(git -C "$work/leddy-mcp-server.rs" rev-parse HEAD^)" = "$LEDDY_MCP_HEAD"
printf 'LEDDY_MCP_EXPECTED_SHA=%s\n' "$LEDDY_MCP_EXPECTED_SHA" >> "$GITHUB_ENV"
cargo +"$rust_toolchain" fmt --manifest-path "$work/leddy-mcp-server.rs/Cargo.toml" --all -- --check
cargo +"$rust_toolchain" clippy --manifest-path "$work/leddy-mcp-server.rs/Cargo.toml" --all-targets --all-features -- -D warnings
cargo +"$rust_toolchain" test --manifest-path "$work/leddy-mcp-server.rs/Cargo.toml" --all-targets --all-features
rm -rf "$work/leddy-sync/target" "$work/leddy-sync/Cargo.lock"
rm -rf "$work/leddy-mcp-server.rs/target" "$work/leddy-mcp-server.rs/Cargo.lock"
test -z "$(git -C "$work/leddy-sync" status --porcelain)"
test -z "$(git -C "$work/leddy-mcp-server.rs" status --porcelain)"

checkout_exact canonical-cloud/.github "$CANONICAL_SOURCE_SHA" "$work/canonical-source"
bash "$work/canonical-source/repository-seeds/canonical-docs-recovery-wave7/reconstruct.sh" \
  "$work/canonical-reconstructed"
cp -a "$work/canonical-reconstructed/extracted/canonical-docs" "$work/canonical-docs"
find "$work/canonical-docs/scripts" -maxdepth 1 -type f -name '*.sh' -exec chmod 755 {} +
(
  cd "$work/canonical-docs"
  python3 scripts/check_docs.py
  python3 -m unittest discover -s tests -v
)
cat > "$work/canonical-message.txt" <<'EOF'
reconstruct canonical-docs from verified Wave 7 source

Source archive SHA-256: 6afa4bef55c3b69b22cc1cad0468d156bce7e7413f3af47addbcca4b25c811c4.
The retained artifact did not include the original .git object database.
This is an explicitly reconstructed commit and does not claim original ancestry.
EOF
sed -i 's/^          //' "$work/canonical-message.txt"
init_reconstructed_repo "$work/canonical-docs" "$work/canonical-message.txt" 2026-08-09T20:00:00Z
CANONICAL_EXPECTED_SHA="$(git -C "$work/canonical-docs" rev-parse HEAD)"
printf 'CANONICAL_EXPECTED_SHA=%s\n' "$CANONICAL_EXPECTED_SHA" >> "$GITHUB_ENV"

checkout_exact evento-globolo/.github "$EVENTO_SOURCE_SHA" "$work/evento-source"
mkdir -p "$work/evgl-e2e"
cp -a "$work/evento-source/repository-seeds/evgl-e2e/." "$work/evgl-e2e/"
rm -f "$work/evgl-e2e/publish.sh"
npm --prefix "$work/evgl-e2e" install --package-lock-only --ignore-scripts --no-audit --no-fund
npm --prefix "$work/evgl-e2e" ci --ignore-scripts --no-audit --no-fund
node "$work/evgl-e2e/scripts/validate-structure.mjs"
node --test "$work/evgl-e2e"/tests/contracts/*.test.mjs
find "$work/evgl-e2e" -type f -name '*.mjs' -not -path '*/node_modules/*' -print0 | xargs -0 -n1 node --check
rm -rf "$work/evgl-e2e/node_modules"
cat > "$work/evento-message.txt" <<'EOF'
bootstrap canonical evgl-e2e harness

Source: evento-globolo/.github@e032dcfcfb70f5798312d2d9eb5f1e570232d543.
The reviewed seed passed structure, contract, browser-matrix, privacy, and baseline checks before publication.
EOF
sed -i 's/^          //' "$work/evento-message.txt"
init_reconstructed_repo "$work/evgl-e2e" "$work/evento-message.txt" 2026-08-09T20:01:00Z
EVENTO_EXPECTED_SHA="$(git -C "$work/evgl-e2e" rev-parse HEAD)"
printf 'EVENTO_EXPECTED_SHA=%s\n' "$EVENTO_EXPECTED_SHA" >> "$GITHUB_ENV"

cat > "$work/hhm-e2e.bundle.b64" <<'HHM_BUNDLE'
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
HHM_BUNDLE
sed -i 's/^          //' "$work/hhm-e2e.bundle.b64"
base64 --decode "$work/hhm-e2e.bundle.b64" > "$work/hhm-e2e.bundle"
verify_bundle "$work/hhm-e2e.bundle" "$HHM_BUNDLE_SHA256" "$HHM_RECOVERED_HEAD" hhm-e2e
git clone --quiet "$work/hhm-e2e.bundle" "$work/hhm-e2e"
test "$(git -C "$work/hhm-e2e" rev-parse HEAD)" = "$HHM_RECOVERED_HEAD"
checkout_exact hacker-house-medellin/.github "$HHM_SOURCE_SHA" "$work/hhm-source"
find "$work/hhm-e2e" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
cp -a "$work/hhm-source/repository-seeds/hhm-e2e/." "$work/hhm-e2e/"
rm -f "$work/hhm-e2e/publish.sh"
cat > "$work/hhm-e2e/RECOVERY-PROVENANCE.md" <<'EOF'
# Recovery provenance

This repository preserves the complete recovered two-commit history ending at
`8f3528add2a3c4f29d50de1a49a4cbddc24e64c9`, then semantically overlays the
reviewed test-organization seed from
`hacker-house-medellin/.github@31f4cac1bbcf89b07ced6a76edcb70d9fd7e02fb`.

The overlay retains the newer Node/browser and Rust-smoke contract matrix while
preserving the original recovered commits. Production promotion remains gated
on this exact target head passing GitHub Actions in `hacker-house-medellin-test`.
EOF
sed -i 's/^          //' "$work/hhm-e2e/RECOVERY-PROVENANCE.md"
test ! -e "$work/hhm-e2e/.zpkg.lock"
test ! -e "$work/hhm-e2e/.gitmodules"
test ! -e "$work/hhm-e2e/rust-smoke/Cargo.lock"
npm --prefix "$work/hhm-e2e" install --package-lock-only --ignore-scripts --no-audit --no-fund
npm --prefix "$work/hhm-e2e" ci --ignore-scripts --no-audit --no-fund
node "$work/hhm-e2e/scripts/validate-structure.mjs"
node --test "$work/hhm-e2e"/tests/contracts/*.test.mjs
cargo generate-lockfile --manifest-path "$work/hhm-e2e/rust-smoke/Cargo.toml"
cargo fmt --manifest-path "$work/hhm-e2e/rust-smoke/Cargo.toml" --all -- --check
cargo check --manifest-path "$work/hhm-e2e/rust-smoke/Cargo.toml" --locked --all-targets
cargo clippy --manifest-path "$work/hhm-e2e/rust-smoke/Cargo.toml" --locked --all-targets -- -D warnings
cargo test --manifest-path "$work/hhm-e2e/rust-smoke/Cargo.toml" --locked
rm -rf "$work/hhm-e2e/node_modules" "$work/hhm-e2e/rust-smoke/target"
if grep -RInE --exclude-dir=.git --exclude-dir=.vendor \
  '(ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|lin_api_[A-Za-z0-9]{20,}|cfat_[A-Za-z0-9_-]{20,}|-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----)' \
  "$work/hhm-e2e"; then
  echo "credential-shaped value found in HHM publication tree" >&2
  exit 19
fi
git -C "$work/hhm-e2e" add --all
GIT_AUTHOR_DATE=2026-08-09T20:02:00Z GIT_COMMITTER_DATE=2026-08-09T20:02:00Z \
  git -C "$work/hhm-e2e" commit --quiet \
    -m "test: reconcile recovered HHM history with reviewed test seed" \
    -m "Preserve exact recovered commits through 8f3528add2a3c4f29d50de1a49a4cbddc24e64c9 and overlay hacker-house-medellin/.github@31f4cac1bbcf89b07ced6a76edcb70d9fd7e02fb."
HHM_EXPECTED_SHA="$(git -C "$work/hhm-e2e" rev-parse HEAD)"
printf 'HHM_EXPECTED_SHA=%s\n' "$HHM_EXPECTED_SHA" >> "$GITHUB_ENV"

for directory in \
  "$work/leddy-sync" \
  "$work/leddy-mcp-server.rs" \
  "$work/canonical-docs" \
  "$work/evgl-e2e" \
  "$work/hhm-e2e"; do
  if grep -RInE --exclude-dir=.git --exclude-dir=.vendor --exclude-dir=target --exclude='package-lock.json' \
    '(ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|lin_api_[A-Za-z0-9]{20,}|cfat_[A-Za-z0-9_-]{20,}|-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----)' \
    "$directory"; then
    echo "credential-shaped value found in $directory" >&2
    exit 20
  fi
done

test "$(git -C "$work/leddy-sync" rev-parse HEAD)" = "$LEDDY_SYNC_HEAD"
test "$(git -C "$work/leddy-mcp-server.rs" rev-parse HEAD)" = "$LEDDY_MCP_EXPECTED_SHA"
test -n "$CANONICAL_EXPECTED_SHA"
test -n "$EVENTO_EXPECTED_SHA"
test -n "$HHM_EXPECTED_SHA"
printf '%s\n' \
  "PREPARED led-dynamo/leddy-sync $LEDDY_SYNC_HEAD" \
  "PREPARED led-dynamo/leddy-mcp-server.rs $LEDDY_MCP_EXPECTED_SHA recovered-parent=$LEDDY_MCP_HEAD" \
  "PREPARED canonical-cloud/canonical-docs $CANONICAL_EXPECTED_SHA" \
  "PREPARED evento-globolo/evgl-e2e $EVENTO_EXPECTED_SHA" \
  "PREPARED hacker-house-medellin-test/hhm-e2e $HHM_EXPECTED_SHA"
