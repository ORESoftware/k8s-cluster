use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const UNKNOWN: &str = "unknown";

// See the engines' build.rs for the prefix convention. This is the web server's
// own release identity (DD_DES_RS_*); the soccer/des engines it embeds carry
// their own SOCCER_ENGINE_* / DES_ENGINE_* identities, surfaced together at
// GET /api/build and GET /info.
const PREFIX: &str = "DD_DES_RS";

fn main() {
    println!("cargo:rerun-if-env-changed={PREFIX}_GIT_COMMIT");
    emit_git_rerun_paths();

    let commit = std::env::var(format!("{PREFIX}_GIT_COMMIT"))
        .ok()
        .and_then(sanitize_commit_label)
        .or_else(|| {
            if repo_git_metadata_exists() {
                git_commit()
            } else {
                None
            }
        })
        .unwrap_or_else(|| UNKNOWN.to_string());

    let commit_date = if repo_git_metadata_exists() {
        git_commit_date().unwrap_or_else(|| UNKNOWN.to_string())
    } else {
        UNKNOWN.to_string()
    };

    let build_timestamp = build_timestamp_iso_utc();

    println!("cargo:rustc-env={PREFIX}_GIT_COMMIT={commit}");
    println!("cargo:rustc-env={PREFIX}_GIT_COMMIT_DATE={commit_date}");
    println!("cargo:rustc-env={PREFIX}_BUILD_TIMESTAMP={build_timestamp}");
}

fn emit_git_rerun_paths() {
    if !repo_git_metadata_exists() {
        return;
    }

    println!("cargo:rerun-if-changed=.git");
    if let Some(path) = git_path("HEAD") {
        println!("cargo:rerun-if-changed={path}");
    }
    if let Some(branch_ref) = git_output(["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git_path(&branch_ref) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(path) = git_path("packed-refs") {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn repo_git_metadata_exists() -> bool {
    Path::new(".git").exists()
}

fn git_commit() -> Option<String> {
    git_output(["rev-parse", "HEAD"]).and_then(sanitize_commit_label)
}

fn git_commit_date() -> Option<String> {
    git_output(["log", "-1", "--format=%cI"]).and_then(sanitize_iso_timestamp)
}

fn git_path(path: &str) -> Option<String> {
    git_output(["rev-parse", "--git-path", path])
}

fn git_output<const N: usize>(args: [&str; N]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if output.status.success() {
        non_empty(String::from_utf8(output.stdout).ok()?)
    } else {
        None
    }
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn sanitize_commit_label(value: String) -> Option<String> {
    let trimmed = value.trim();
    let first = trimmed.split_ascii_whitespace().next()?;
    if first.is_empty() || first.len() > 80 {
        return None;
    }
    if first
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
    {
        Some(first.to_string())
    } else {
        None
    }
}

fn sanitize_iso_timestamp(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 40 {
        return None;
    }
    if trimmed
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '-' | ':' | 'T' | '+' | 'Z' | '.'))
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn build_timestamp_iso_utc() -> String {
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });
    unix_seconds_to_iso_utc(secs)
}

fn unix_seconds_to_iso_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
