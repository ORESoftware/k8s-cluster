use std::{
    env,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_or_git(env_key: &str, git_args: &[&str], fallback: &str) -> String {
    env::var(env_key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| git_output(git_args))
        .unwrap_or_else(|| fallback.to_string())
}

fn build_time() -> String {
    env::var("SOURCE_DATE_EPOCH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        })
}

fn git_dirty() -> String {
    env::var("DD_GIT_DIRTY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(
            || match git_output(&["status", "--porcelain", "--untracked-files=no"]) {
                Some(value) if value.is_empty() => "false".to_string(),
                Some(_) => "true".to_string(),
                None => "unknown".to_string(),
            },
        )
}

fn flags2env_vendor_dir() -> PathBuf {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    let direct = manifest_dir.join("third_party/flags2env");
    if direct.join("src/parser.c").is_file() {
        return direct;
    }

    manifest_dir
        .parent()
        .map(|parent| parent.join("third_party/flags2env"))
        .unwrap_or(direct)
}

fn compile_flags2env_parser() {
    let vendor_dir = flags2env_vendor_dir();
    let source_dir = vendor_dir.join("src");
    let parser = source_dir.join("parser.c");
    let header = source_dir.join("parser.h");

    println!("cargo:rerun-if-changed={}", parser.display());
    println!("cargo:rerun-if-changed={}", header.display());

    if !parser.is_file() || !header.is_file() {
        panic!(
            "missing vendored flags2env C parser; expected {} and {}. Make sure third_party/flags2env is present when building or installing this crate.",
            parser.display(),
            header.display()
        );
    }

    cc::Build::new()
        .file(parser)
        .include(source_dir)
        .flag_if_supported("-std=c99")
        .compile("flags2env_parser");
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-env-changed=DD_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=DD_GIT_COMMIT_SHORT");
    println!("cargo:rerun-if-env-changed=DD_GIT_REF");
    println!("cargo:rerun-if-env-changed=DD_GIT_DIRTY");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    compile_flags2env_parser();

    let commit = env_or_git("DD_GIT_COMMIT", &["rev-parse", "HEAD"], "unknown");
    let short = env_or_git(
        "DD_GIT_COMMIT_SHORT",
        &["rev-parse", "--short", "HEAD"],
        "unknown",
    );
    let git_ref = env_or_git(
        "DD_GIT_REF",
        &["rev-parse", "--abbrev-ref", "HEAD"],
        "unknown",
    );

    println!("cargo:rustc-env=DD_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=DD_GIT_COMMIT_SHORT={short}");
    println!("cargo:rustc-env=DD_GIT_REF={git_ref}");
    println!("cargo:rustc-env=DD_GIT_DIRTY={}", git_dirty());
    println!("cargo:rustc-env=DD_BUILD_TIME_UTC={}", build_time());
}
