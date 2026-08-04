#!/usr/bin/env python3
"""Apply the reviewed DEN-539 fixed-profile mapping to exact source blobs."""

from __future__ import annotations

import subprocess
from pathlib import Path

PROFILES = Path("remote/deployments/build-server-rs/src/profiles.rs")
PLANNER = Path("remote/deployments/gha-clone-server-rs/src/lib.rs")


def git_blob(path: Path) -> str:
    return subprocess.check_output(
        ["git", "hash-object", str(path)], text=True
    ).strip()


def require_blob(path: Path, expected: str) -> str:
    observed = git_blob(path)
    if observed != expected:
        raise SystemExit(
            f"refusing drifted {path}: expected {expected}, observed {observed}"
        )
    return path.read_text(encoding="utf-8")


def replace_once(source: str, old: str, new: str, label: str) -> str:
    if source.count(old) != 1:
        raise SystemExit(f"{label} anchor was not unique")
    return source.replace(old, new, 1)


def patch_profiles() -> None:
    current = PROFILES.read_text(encoding="utf-8")
    if "RUST_GENERATED_VERIFY_STEPS" in current:
        return
    source = require_blob(PROFILES, "f0e2459e5a8233301b8fbd3b142398e080e2d96e")

    generated_constant = '''const RUST_GENERATED_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {
    name: "Generated Rust interface formatting, Clippy, and tests",
    image: RUST_IMAGE,
    subdirectory: ".",
    script: r#"set -euo pipefail
manifest=generated/rust/Cargo.toml
if [ ! -f "$manifest" ]; then
  echo "rust-generated-verify requires generated/rust/Cargo.toml" >&2
  exit 2
fi
cargo generate-lockfile --manifest-path "$manifest"
rustup component add rustfmt clippy
cargo fmt --manifest-path "$manifest" -- --check
cargo clippy --locked --manifest-path "$manifest" --all-targets -- -D warnings
cargo test --locked --manifest-path "$manifest" --all-targets"#,
}];

'''
    source = replace_once(
        source,
        "const NODE_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {",
        generated_constant + "const NODE_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {",
        "profile constant",
    )

    generated_spec = '''    ProfileSpec {
        name: "rust-generated-verify",
        platform: "linux",
        description: "Generated Rust interface lock, formatting, warnings-denied Clippy, and tests",
        steps: RUST_GENERATED_VERIFY_STEPS,
        artifact_paths: &[],
    },
'''
    source = replace_once(
        source,
        '''    ProfileSpec {
        name: "node-verify",
''',
        generated_spec
        + '''    ProfileSpec {
        name: "node-verify",
''',
        "profile spec",
    )
    source = replace_once(
        source,
        '''            "rust-verify",
            "node-verify",
''',
        '''            "rust-verify",
            "rust-generated-verify",
            "node-verify",
''',
        "installed profile test",
    )

    generated_test = '''    #[test]
    fn generated_rust_profile_is_manifest_scoped_and_ordered() {
        let profile = find("rust-generated-verify").expect("generated Rust profile");
        let script = profile.steps[0].script;
        let lock = script.find("cargo generate-lockfile").expect("lock step");
        let fmt = script.find("cargo fmt").expect("format step");
        let clippy = script.find("cargo clippy --locked").expect("Clippy step");
        let test = script.find("cargo test --locked").expect("test step");
        assert!(lock < fmt && fmt < clippy && clippy < test);
        assert_eq!(profile.steps[0].subdirectory, ".");
        assert!(script.contains("manifest=generated/rust/Cargo.toml"));
        assert!(!script.contains("cargo publish"));
        assert!(!script.contains("find "));
        assert!(!script.contains("|| true"));
    }

'''
    source = replace_once(
        source,
        '''    #[test]
    fn rust_verify_has_only_the_reviewed_meta_server_monorepo_fallback() {
''',
        generated_test
        + '''    #[test]
    fn rust_verify_has_only_the_reviewed_meta_server_monorepo_fallback() {
''',
        "generated profile test",
    )
    PROFILES.write_text(source, encoding="utf-8")


def patch_planner() -> None:
    current = PLANNER.read_text(encoding="utf-8")
    if "fn generated_rust_profile" in current:
        return
    source = require_blob(PLANNER, "da2140766c98e43e0d1aa6afc9893e94f9e3bfa0")
    source = replace_once(
        source,
        '            "rust-verify".to_string(),\n',
        '            "rust-verify".to_string(),\n            "rust-generated-verify".to_string(),\n',
        "capability profile",
    )

    source = replace_once(
        source,
        '''    let profile = if hardened_node_intent(&lower) {
''',
        '''    let profile = if generated_rust_intent(&lower) {
        match generated_rust_profile(&run_commands) {
            Some(profile) => Some(profile.to_string()),
            None => {
                reasons.push(
                    "generated Rust jobs must use one exact reviewed command sequence in the documented order with no extra commands"
                        .into(),
                );
                None
            }
        }
    } else if hardened_node_intent(&lower) {
''',
        "planner classification",
    )

    generated_helpers = '''fn generated_rust_intent(text: &str) -> bool {
    text.contains("generated/rust/cargo.toml")
}

fn generated_rust_profile(commands: &[String]) -> Option<&'static str> {
    const EXACT: [&str; 4] = [
        "cargo generate-lockfile --manifest-path generated/rust/Cargo.toml",
        "cargo fmt --manifest-path generated/rust/Cargo.toml -- --check",
        "cargo clippy --locked --manifest-path generated/rust/Cargo.toml --all-targets -- -D warnings",
        "cargo test --locked --manifest-path generated/rust/Cargo.toml --all-targets",
    ];
    commands
        .iter()
        .map(String::as_str)
        .eq(EXACT)
        .then_some("rust-generated-verify")
}

'''
    source = replace_once(
        source,
        "fn hardened_node_intent(text: &str) -> bool {\n",
        generated_helpers + "fn hardened_node_intent(text: &str) -> bool {\n",
        "planner helper",
    )

    final_close = source.rfind("\n}")
    if final_close < 0:
        raise SystemExit("planner test-module close was not found")
    generated_test = '''

    #[test]
    fn generated_rust_commands_are_exact_and_order_sensitive() {
        let exact = [
            "cargo generate-lockfile --manifest-path generated/rust/Cargo.toml",
            "cargo fmt --manifest-path generated/rust/Cargo.toml -- --check",
            "cargo clippy --locked --manifest-path generated/rust/Cargo.toml --all-targets -- -D warnings",
            "cargo test --locked --manifest-path generated/rust/Cargo.toml --all-targets",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        assert_eq!(generated_rust_profile(&exact), Some("rust-generated-verify"));

        let mut reordered = exact.clone();
        reordered.swap(2, 3);
        assert_eq!(generated_rust_profile(&reordered), None);

        let mut extra = exact.clone();
        extra.push("cargo publish --manifest-path generated/rust/Cargo.toml".into());
        assert_eq!(generated_rust_profile(&extra), None);
        assert!(generated_rust_intent(&exact.join("\n").to_ascii_lowercase()));
    }
'''
    source = source[:final_close] + generated_test + source[final_close:]
    PLANNER.write_text(source, encoding="utf-8")


def main() -> None:
    patch_profiles()
    patch_planner()
    print("DEN-539 fixed profile and planner mapping applied")


if __name__ == "__main__":
    main()
