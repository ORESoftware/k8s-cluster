from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def replace_count(text: str, old: str, new: str, count: int, label: str) -> str:
    actual = text.count(old)
    if actual != count:
        raise RuntimeError(f"{label}: expected {count} matches, found {actual}")
    return text.replace(old, new)


jobs = Path("remote/deployments/build-server-rs/src/jobs.rs")
text = jobs.read_text(encoding="utf-8")
old = '''pub(crate) async fn clone_repository(
    config: &Config,
    request: &BuildRequest,
    job_dir: &Path,
    repo_dir: &Path,
    log_path: &Path,
) -> Result<(), String> {
    let mut clone_args = vec![
        "-c".to_string(),
        "protocol.ext.allow=never".to_string(),
        "-c".to_string(),
        "protocol.file.allow=never".to_string(),
        "-c".to_string(),
        "protocol.local.allow=never".to_string(),
        "clone".to_string(),
        "--depth".to_string(),
        "1".to_string(),
        "--no-tags".to_string(),
    ];
    if let Some(git_ref) = clean_optional(request.git_ref.as_deref()) {
        clone_args.push("--branch".to_string());
        clone_args.push(git_ref);
    }
    clone_args.push("--".to_string());
    clone_args.push(request.repo_url.clone());
    clone_args.push(repo_dir.to_string_lossy().to_string());
    run_logged_command(config, log_path, job_dir, &config.git_bin, clone_args).await
}
'''
new = '''fn locked_git_args(args: Vec<String>) -> Vec<String> {
    let mut command = vec![
        "-c".to_string(),
        "protocol.ext.allow=never".to_string(),
        "-c".to_string(),
        "protocol.file.allow=never".to_string(),
        "-c".to_string(),
        "protocol.local.allow=never".to_string(),
    ];
    command.extend(args);
    command
}

fn is_immutable_commit_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn clone_repository_commands(request: &BuildRequest, repo_dir: &Path) -> Vec<Vec<String>> {
    let destination = repo_dir.to_string_lossy().to_string();
    match clean_optional(request.git_ref.as_deref()) {
        Some(git_ref) if is_immutable_commit_sha(&git_ref) => vec![
            locked_git_args(vec![
                "init".to_string(),
                "--".to_string(),
                destination.clone(),
            ]),
            locked_git_args(vec![
                "-C".to_string(),
                destination.clone(),
                "remote".to_string(),
                "add".to_string(),
                "origin".to_string(),
                "--".to_string(),
                request.repo_url.clone(),
            ]),
            locked_git_args(vec![
                "-C".to_string(),
                destination.clone(),
                "fetch".to_string(),
                "--depth".to_string(),
                "1".to_string(),
                "--no-tags".to_string(),
                "--".to_string(),
                "origin".to_string(),
                git_ref.clone(),
            ]),
            locked_git_args(vec![
                "-C".to_string(),
                destination.clone(),
                "checkout".to_string(),
                "--detach".to_string(),
                git_ref.clone(),
            ]),
            locked_git_args(vec![
                "-C".to_string(),
                destination,
                "rev-parse".to_string(),
                "--verify".to_string(),
                format!("{git_ref}^{{commit}}"),
            ]),
        ],
        git_ref => {
            let mut args = vec![
                "clone".to_string(),
                "--depth".to_string(),
                "1".to_string(),
                "--no-tags".to_string(),
            ];
            if let Some(git_ref) = git_ref {
                args.push("--branch".to_string());
                args.push(git_ref);
            }
            args.extend([
                "--".to_string(),
                request.repo_url.clone(),
                destination,
            ]);
            vec![locked_git_args(args)]
        }
    }
}

pub(crate) async fn clone_repository(
    config: &Config,
    request: &BuildRequest,
    job_dir: &Path,
    repo_dir: &Path,
    log_path: &Path,
) -> Result<(), String> {
    for args in clone_repository_commands(request, repo_dir) {
        run_logged_command(config, log_path, job_dir, &config.git_bin, args).await?;
    }
    Ok(())
}
'''
text = replace_once(text, old, new, "immutable clone implementation")

if "#[cfg(test)]\nmod tests" in text:
    raise RuntimeError("jobs.rs unexpectedly already contains a test module")

tests = r'''

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        process::Command as StdCommand,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn request(repo_url: String, git_ref: Option<&str>) -> BuildRequest {
        BuildRequest {
            schema_version: Some("build-server.v1".to_string()),
            job_kind: Some("run-profile".to_string()),
            repo_url,
            git_ref: git_ref.map(ToString::to_string),
            image: String::new(),
            profile: Some("node-verify".to_string()),
            context_dir: None,
            dockerfile: None,
            build_args: None,
            push: None,
            deploy: None,
            executor: None,
            request_id: None,
        }
    }

    fn run_git(cwd: &Path, args: &[&str]) -> String {
        let output = StdCommand::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap_or_else(|error| panic!("spawn git {args:?}: {error}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("git stdout must be UTF-8")
            .trim()
            .to_string()
    }

    fn run_planned_git(cwd: &Path, mut args: Vec<String>) {
        let mut command_index = 0;
        while args.get(command_index).map(String::as_str) == Some("-c") {
            command_index += 2;
        }
        // The production plan deliberately blocks local transports. This test
        // uses a temporary local bare remote and overrides only that transport
        // after asserting the lockdown is present in the generated plan.
        args.splice(
            command_index..command_index,
            [
                "-c".to_string(),
                "protocol.file.allow=always".to_string(),
                "-c".to_string(),
                "protocol.local.allow=always".to_string(),
            ],
        );
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        let _ = run_git(cwd, &refs);
    }

    #[test]
    fn immutable_commit_detection_is_exact_and_case_insensitive() {
        assert!(is_immutable_commit_sha(
            "7d905806b2000479bdacb9b206f33b26a707ba5e"
        ));
        assert!(is_immutable_commit_sha(
            "7D905806B2000479BDACB9B206F33B26A707BA5E"
        ));
        assert!(!is_immutable_commit_sha("main"));
        assert!(!is_immutable_commit_sha(
            "7d905806b2000479bdacb9b206f33b26a707ba5"
        ));
        assert!(!is_immutable_commit_sha(
            "zd905806b2000479bdacb9b206f33b26a707ba5e"
        ));
    }

    #[test]
    fn immutable_clone_plan_fetches_and_detaches_without_branch_fallback() {
        let revision = "7d905806b2000479bdacb9b206f33b26a707ba5e";
        let commands = clone_repository_commands(
            &request(
                "https://github.com/messaging-intel/msgint-connectors.git".to_string(),
                Some(revision),
            ),
            Path::new("/tmp/msgint-checkout"),
        );
        assert_eq!(commands.len(), 5);
        let flattened = commands.concat();
        assert!(flattened.contains(&"protocol.ext.allow=never".to_string()));
        assert!(flattened.contains(&"protocol.file.allow=never".to_string()));
        assert!(flattened.contains(&"protocol.local.allow=never".to_string()));
        assert!(flattened.contains(&"fetch".to_string()));
        assert!(flattened.contains(&"checkout".to_string()));
        assert!(flattened.contains(&"--detach".to_string()));
        assert!(flattened.contains(&revision.to_string()));
        assert!(!flattened.contains(&"--branch".to_string()));

        let mutable = clone_repository_commands(
            &request(
                "https://github.com/ORESoftware/k8s-cluster.git".to_string(),
                Some("dev"),
            ),
            Path::new("/tmp/mutable-checkout"),
        );
        assert_eq!(mutable.len(), 1);
        assert!(mutable[0].windows(2).any(|pair| pair == ["--branch", "dev"]));
    }

    #[test]
    fn immutable_clone_plan_checks_out_the_requested_non_tip_commit() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "dd-build-server-immutable-clone-{}-{nonce}",
            std::process::id()
        ));
        let source = root.join("source");
        let remote = root.join("remote.git");
        let checkout = root.join("checkout");
        fs::create_dir_all(&root).expect("create test root");

        let root_text = root.to_string_lossy().to_string();
        let source_text = source.to_string_lossy().to_string();
        let remote_text = remote.to_string_lossy().to_string();
        run_git(&root, &["init", "--", &source_text]);
        run_git(&source, &["config", "user.email", "gha-clone@example.invalid"]);
        run_git(&source, &["config", "user.name", "GHA clone test"]);
        fs::write(source.join("payload.txt"), "first\n").expect("write first payload");
        run_git(&source, &["add", "--", "payload.txt"]);
        run_git(&source, &["commit", "-m", "first"]);
        let requested = run_git(&source, &["rev-parse", "HEAD"]);

        fs::write(source.join("payload.txt"), "tip\n").expect("write tip payload");
        run_git(&source, &["add", "--", "payload.txt"]);
        run_git(&source, &["commit", "-m", "tip"]);
        let tip = run_git(&source, &["rev-parse", "HEAD"]);
        assert_ne!(requested, tip);
        run_git(&root, &["clone", "--bare", "--", &source_text, &remote_text]);

        let plan = clone_repository_commands(
            &request(format!("file://{remote_text}"), Some(&requested)),
            &checkout,
        );
        assert_eq!(plan.len(), 5);
        for command in &plan {
            assert!(command.contains(&"protocol.file.allow=never".to_string()));
            assert!(command.contains(&"protocol.local.allow=never".to_string()));
        }
        for command in plan {
            run_planned_git(&root, command);
        }

        assert_eq!(run_git(&checkout, &["rev-parse", "HEAD"]), requested);
        assert_eq!(
            fs::read_to_string(checkout.join("payload.txt")).expect("read checked-out payload"),
            "first\n"
        );
        fs::remove_dir_all(&root)
            .unwrap_or_else(|error| panic!("remove temporary tree {root_text}: {error}"));
    }
}
'''
text = text.rstrip() + tests + "\n"
jobs.write_text(text, encoding="utf-8")

workflow = Path(".github/workflows/gha-clone-server.yml")
text = workflow.read_text(encoding="utf-8")
text = replace_count(
    text,
    "      - 'remote/deployments/build-server-rs/src/profiles.rs'",
    "      - 'remote/deployments/build-server-rs/src/**'",
    2,
    "build-server workflow path filters",
)
text = replace_once(
    text,
    '''      - name: Check the modified fixed-profile source formatting
        working-directory: remote/deployments/build-server-rs
        run: rustfmt --edition 2021 --check src/profiles.rs
      - name: Compile the build server against the exact private library gitlink
        working-directory: remote/deployments/build-server-rs
        run: cargo check --locked --all-targets
      - name: Test the fixed-profile registry and meta fallback
        working-directory: remote/deployments/build-server-rs
        run: cargo test --locked profiles::tests -- --nocapture
''',
    '''      - name: Check build-server formatting
        working-directory: remote/deployments/build-server-rs
        run: cargo fmt --all -- --check
      - name: Compile the build server against the exact private library gitlink
        working-directory: remote/deployments/build-server-rs
        run: cargo check --locked --all-targets
      - name: Test the fixed profiles, allowlists, and immutable checkout path
        working-directory: remote/deployments/build-server-rs
        run: cargo test --locked --all-targets -- --nocapture
''',
    "build-server validation steps",
)
workflow.write_text(text, encoding="utf-8")

readme = Path("remote/deployments/build-server-rs/readme.md")
text = readme.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''```\n\nEach profile container is CPU-, memory-, PID-, capability-, and privilege-limited.''',
    '''```\n\nA 40-hex `gitRef` is treated as an immutable commit identity: the server initializes an empty checkout, fetches that exact object with depth one, and checks it out detached. It never passes a commit SHA to `git clone --branch` and never falls back to a mutable branch or default-branch tip. Human-readable branch and tag names continue to use the bounded shallow-clone path.\n\nEach profile container is CPU-, memory-, PID-, capability-, and privilege-limited.''',
    "immutable checkout documentation",
)
readme.write_text(text, encoding="utf-8")
