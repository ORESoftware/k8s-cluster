from pathlib import Path

PLANNER = Path("remote/deployments/gha-clone-server-rs/src/lib.rs")
TEMPORARY_PATHS = (
    Path(".github/workflows/apply-den-539-threefa-planner-once.yml"),
    Path(".github/workflows/apply-den-539-threefa-planner-push-once.yml"),
    Path("scripts/ops/apply_den_539_threefa_planner.py"),
)


def replace_once(source: str, old: str, new: str, label: str) -> str:
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one marker, found {count}")
    return source.replace(old, new, 1)


def main() -> None:
    source = PLANNER.read_text(encoding="utf-8")

    replacements = [
        (
            '''            "rust-verify".to_string(),
            "node-verify".to_string(),''',
            '''            "rust-verify".to_string(),
            "rust-generated-verify".to_string(),
            "node-verify".to_string(),
            "node-hardened-verify".to_string(),
            "node-hardened-test".to_string(),''',
            "capability profiles",
        ),
        (
            '''    let mut combined = String::new();
    let has_services = mapping_get(job, "services").is_some();''',
            '''    let mut combined = String::new();
    let mut run_commands = Vec::<String>::new();
    let has_services = mapping_get(job, "services").is_some();''',
            "run command accumulator",
        ),
        (
            '''        if let Some(run) = mapping_get(step, "run").and_then(Value::as_str) {
            combined.push_str(run);
            combined.push('\n');
            if run.contains("${{") {''',
            '''        if let Some(run) = mapping_get(step, "run").and_then(Value::as_str) {
            combined.push_str(run);
            combined.push('\n');
            run_commands.extend(
                run.lines()
                    .map(str::trim)
                    .filter(|command| !command.is_empty())
                    .map(str::to_string),
            );
            if run.contains("${{") {''',
            "run command extraction",
        ),
        (
            '''    let lower = combined.to_ascii_lowercase();
    let profile = classify_profile(&lower);
    if profile.is_none() {
        reasons.push("no fixed build-server profile matches this job".into());
    }''',
            '''    let lower = combined.to_ascii_lowercase();
    let generated_rust = generated_rust_intent(&lower);
    let hardened_node = hardened_node_intent(&lower);
    let profile = if generated_rust {
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
    } else if hardened_node {
        match hardened_node_profile(&run_commands) {
            Some(profile) => Some(profile.to_string()),
            None => {
                reasons.push(
                    "hardened Node jobs must use one exact reviewed command sequence in the documented order with no extra commands"
                        .into(),
                );
                None
            }
        }
    } else {
        classify_profile(&lower)
    };
    if profile.is_none() && !generated_rust && !hardened_node {
        reasons.push("no fixed build-server profile matches this job".into());
    }''',
            "exact profile classification",
        ),
        (
            '''fn classify_profile(text: &str) -> Option<String> {''',
            '''fn generated_rust_intent(text: &str) -> bool {
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

fn hardened_node_intent(text: &str) -> bool {
    text.contains("npm ci --ignore-scripts") || text.contains("npm run test:operator-config")
}

fn hardened_node_profile(commands: &[String]) -> Option<&'static str> {
    const VERIFY: [&str; 4] = [
        "npm ci --ignore-scripts",
        "npm run check",
        "npm run test:operator-config",
        "npm audit --audit-level=high",
    ];
    const TEST: [&str; 2] = ["npm ci --ignore-scripts", "npm test"];
    if commands.iter().map(String::as_str).eq(VERIFY) {
        Some("node-hardened-verify")
    } else if commands.iter().map(String::as_str).eq(TEST) {
        Some("node-hardened-test")
    } else {
        None
    }
}

fn classify_profile(text: &str) -> Option<String> {''',
            "exact profile helpers",
        ),
        (
            '''    #[test]
    fn verifies_github_hmac_sha256() {''',
            '''    #[test]
    fn hardened_and_generated_command_sequences_are_exact() {
        let node_test = ["npm ci --ignore-scripts", "npm test"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(hardened_node_profile(&node_test), Some("node-hardened-test"));
        let mut extra_node = node_test.clone();
        extra_node.push("npm audit --audit-level=high".into());
        assert_eq!(hardened_node_profile(&extra_node), None);

        let generated = [
            "cargo generate-lockfile --manifest-path generated/rust/Cargo.toml",
            "cargo fmt --manifest-path generated/rust/Cargo.toml -- --check",
            "cargo clippy --locked --manifest-path generated/rust/Cargo.toml --all-targets -- -D warnings",
            "cargo test --locked --manifest-path generated/rust/Cargo.toml --all-targets",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        assert_eq!(
            generated_rust_profile(&generated),
            Some("rust-generated-verify")
        );
        let mut reordered = generated.clone();
        reordered.swap(2, 3);
        assert_eq!(generated_rust_profile(&reordered), None);
    }

    #[test]
    fn verifies_github_hmac_sha256() {''',
            "exact profile unit tests",
        ),
    ]

    for old, new, label in replacements:
        source = replace_once(source, old, new, label)

    PLANNER.write_text(source, encoding="utf-8")
    for path in TEMPORARY_PATHS:
        path.unlink()


if __name__ == "__main__":
    main()
