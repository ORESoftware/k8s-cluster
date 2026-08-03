use std::process::{Command, Output};

fn run_server_with(arg: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_threefa-sync-server"))
        .arg(arg)
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
    let output = run_server_with(&argument);
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
fn arbitrary_unknown_flags_fail_before_server_startup() {
    let output = run_server_with("--definitely-not-a-threefa-option=1");
    let stderr = assert_rejected_without_stdout(&output);

    assert!(stderr.contains("invalid command-line configuration"));
    assert!(stderr.contains("unknown command-line option"));
}
