use std::process::{Command, Output};

fn run_server_with(arg: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_threefa-sync-server"))
        .arg(arg)
        .output()
        .expect("threefa-sync-server should start far enough to validate CLI flags")
}

#[test]
fn secret_bearing_database_url_is_not_accepted_as_a_cli_flag() {
    let output = run_server_with("--database-url=postgres://redacted.invalid/threefa");

    assert_eq!(output.status.code(), Some(2), "unexpected status: {output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown command-line option"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn arbitrary_unknown_flags_fail_before_server_startup() {
    let output = run_server_with("--definitely-not-a-threefa-option=1");

    assert_eq!(output.status.code(), Some(2), "unexpected status: {output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid command-line configuration"));
    assert!(stderr.contains("unknown command-line option"));
    assert!(output.stdout.is_empty(), "CLI errors must not write to stdout");
}
