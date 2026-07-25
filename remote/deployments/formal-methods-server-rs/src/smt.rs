use std::{
    collections::{BTreeMap, HashSet},
    process::Stdio,
};

use tokio::{io::AsyncWriteExt, process::Command, time::timeout};

use crate::{annotations::VarDecl, expr::SortHint, state::Config};

// ---------------------------------------------------------------------------
// SMT scripting
// ---------------------------------------------------------------------------

fn sort_string(sort: &SortHint) -> &'static str {
    match sort {
        SortHint::Real => "Real",
        SortHint::Bool => "Bool",
        SortHint::Int | SortHint::Unknown => "Int",
    }
}

pub(crate) fn declarations_for(decls: &[VarDecl]) -> String {
    let mut buf = String::new();
    for decl in decls {
        buf.push_str(&format!(
            "(declare-const {} {})\n",
            decl.name,
            sort_string(&decl.sort)
        ));
    }
    buf
}

pub(crate) fn declarations_with_extras(decls: &[VarDecl], extra_vars: &HashSet<String>) -> String {
    let declared: HashSet<&str> = decls.iter().map(|d| d.name.as_str()).collect();
    let mut buf = declarations_for(decls);
    for name in extra_vars {
        if !declared.contains(name.as_str()) {
            buf.push_str(&format!("(declare-const {name} Int)\n"));
        }
    }
    buf
}

#[derive(Debug, Clone)]
pub(crate) struct SmtResult {
    pub(crate) status: SmtStatus,
    pub(crate) model: BTreeMap<String, String>,
    pub(crate) raw: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SmtStatus {
    Sat,
    Unsat,
    Unknown,
    Error,
}

pub(crate) async fn run_z3(config: &Config, script: &str) -> Result<SmtResult, String> {
    let mut child = Command::new(&config.z3_bin)
        .args(["-in", "-smt2", "-T:5"])
        .env_clear()
        .env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("failed to spawn {}: {error}", config.z3_bin))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(script.as_bytes())
            .await
            .map_err(|error| format!("failed to write to z3 stdin: {error}"))?;
        drop(stdin);
    }

    let output = match timeout(config.z3_timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(error)) => return Err(format!("z3 wait failed: {error}")),
        Err(_) => return Err(format!("z3 timed out after {:?}", config.z3_timeout)),
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let raw = if stderr.trim().is_empty() {
        stdout.clone()
    } else {
        format!("{stdout}---\n{stderr}")
    };
    let trimmed = stdout.trim();
    let first_line = trimmed.lines().next().unwrap_or("").trim();
    let status = match first_line {
        "sat" => SmtStatus::Sat,
        "unsat" => SmtStatus::Unsat,
        "unknown" => SmtStatus::Unknown,
        _ => SmtStatus::Error,
    };
    let model = if status == SmtStatus::Sat {
        parse_model(trimmed)
    } else {
        BTreeMap::new()
    };
    Ok(SmtResult { status, model, raw })
}

fn parse_model(output: &str) -> BTreeMap<String, String> {
    // Z3 emits `(get-model)` results as one or more `(define-fun NAME () SORT VALUE)`
    // entries, often broken across multiple lines (e.g. `Int\n    0)`). We
    // walk the byte buffer once, locating each define-fun, then read a
    // paren-balanced VALUE until the matching closing paren of the binding.
    let mut model = BTreeMap::new();
    let bytes = output.as_bytes();
    let needle = b"(define-fun ";
    let mut i = 0usize;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] != needle {
            i += 1;
            continue;
        }
        let mut j = i + needle.len();
        let name_start = j;
        while j < bytes.len() && !(bytes[j] as char).is_whitespace() {
            j += 1;
        }
        let name = output[name_start..j].to_string();
        while j < bytes.len() && (bytes[j] as char).is_whitespace() {
            j += 1;
        }
        if j + 2 <= bytes.len() && bytes[j] == b'(' && bytes[j + 1] == b')' {
            j += 2;
        }
        while j < bytes.len() && (bytes[j] as char).is_whitespace() {
            j += 1;
        }
        while j < bytes.len() && !(bytes[j] as char).is_whitespace() {
            j += 1;
        }
        while j < bytes.len() && (bytes[j] as char).is_whitespace() {
            j += 1;
        }
        let val_start = j;
        let mut depth: i32 = 0;
        while j < bytes.len() {
            let c = bytes[j];
            if c == b'(' {
                depth += 1;
                j += 1;
            } else if c == b')' {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                j += 1;
            } else {
                j += 1;
            }
        }
        let raw_value = &output[val_start..j];
        let cleaned = raw_value.split_whitespace().collect::<Vec<_>>().join(" ");
        if !name.is_empty() && !cleaned.is_empty() {
            model.insert(name, cleaned);
        }
        i = j;
    }
    model
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_model_handles_multiline_z3_output() {
        let raw = "sat\n(\n  (define-fun y () Int\n    0)\n  (define-fun x () Int\n    (- 3))\n)\n";
        let model = parse_model(raw);
        assert_eq!(model.get("y").map(String::as_str), Some("0"));
        assert_eq!(model.get("x").map(String::as_str), Some("(- 3)"));
    }
}
