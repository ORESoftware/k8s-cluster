use std::process::{Command, Output};

fn run_server_with(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_threefa-sync-server"))
        .args(args)
        .output()
        .expect("threefa-sync-server should start far enough to validate CLI flags")
}

fn assert_rejected_without_stdout(output: &Output) -> String {
    assert_eq!(
        output.status.code(),
        Some(2),
        "unexpected status: {output:?}"
    );
    assert!(
        output.stdout.is_empty(),
        "CLI errors must not write to stdout"
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn secret_bearing_database_url_is_rejected_without_echoing_the_secret() {
    const SECRET: &str = "postgres://must-remain-environment-only@redacted.invalid/threefa";
    let argument = format!("--database-url={SECRET}");
    let output = run_server_with(&[argument.as_str()]);
    let stderr = assert_rejected_without_stdout(&output);

    assert!(
        stderr.contains("unknown command-line option"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !stderr.contains(SECRET) && !stderr.contains("must-remain-environment-only"),
        "rejected secret-bearing argument was echoed: {stderr}"
    );
}

#[test]
fn split_secret_bearing_database_url_is_rejected_without_echoing_the_value() {
    const SECRET: &str = "postgres://split-secret@redacted.invalid/threefa";
    let output = run_server_with(&["--database-url", SECRET]);
    let stderr = assert_rejected_without_stdout(&output);

    assert!(stderr.contains("unknown command-line option"));
    assert!(stderr.contains("--database-url"));
    assert!(
        !stderr.contains(SECRET) && !stderr.contains("split-secret"),
        "split rejected value was echoed: {stderr}"
    );
}

#[test]
fn malformed_secret_bearing_option_token_is_fully_redacted() {
    const SECRET: &str = "embedded-secret";
    let argument = format!("--postgres://{SECRET}@redacted.invalid/threefa");
    let output = run_server_with(&[argument.as_str()]);
    let stderr = assert_rejected_without_stdout(&output);

    assert!(stderr.contains("unknown command-line option"));
    assert!(stderr.contains("<redacted-option>"));
    assert!(
        !stderr.contains(SECRET),
        "malformed rejected option token was echoed: {stderr}"
    );
}

#[test]
fn arbitrary_unknown_flags_fail_before_server_startup() {
    let output = run_server_with(&["--definitely-not-a-threefa-option=1"]);
    let stderr = assert_rejected_without_stdout(&output);

    assert!(stderr.contains("invalid command-line configuration"));
    assert!(stderr.contains("unknown command-line option"));
}
