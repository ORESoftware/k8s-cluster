use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::{
    state::{AppState, Config},
    types::{Finding, PullRequestRef, Severity},
    SERVICE_NAME,
};

// ---------------------------------------------------------------------------
// GitHub: webhook HMAC + PR comment posting
// ---------------------------------------------------------------------------

type HmacSha256 = Hmac<Sha256>;

pub(crate) fn verify_github_signature(secret: &str, body: &[u8], header_value: &str) -> bool {
    let prefix = "sha256=";
    if !header_value.starts_with(prefix) {
        return false;
    }
    let provided_hex = &header_value[prefix.len()..];
    let Ok(provided) = hex::decode(provided_hex) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&provided).is_ok()
}

pub(crate) fn extract_pr_from_event(payload: &Value) -> Result<PullRequestRef, String> {
    let pr = payload
        .get("pull_request")
        .ok_or_else(|| "payload is missing pull_request".to_string())?;
    let repository = payload
        .get("repository")
        .ok_or_else(|| "payload is missing repository".to_string())?;

    let full_name = repository
        .get("full_name")
        .and_then(Value::as_str)
        .ok_or_else(|| "repository.full_name missing".to_string())?;
    let (owner, repo) = full_name
        .split_once('/')
        .ok_or_else(|| "repository.full_name is not owner/repo".to_string())?;
    let number = pr
        .get("number")
        .and_then(Value::as_u64)
        .ok_or_else(|| "pull_request.number missing".to_string())?;

    let head = pr
        .get("head")
        .ok_or_else(|| "pull_request.head missing".to_string())?;
    let base = pr
        .get("base")
        .ok_or_else(|| "pull_request.base missing".to_string())?;
    let head_sha = head
        .get("sha")
        .and_then(Value::as_str)
        .ok_or_else(|| "pull_request.head.sha missing".to_string())?;
    let base_sha = base
        .get("sha")
        .and_then(Value::as_str)
        .ok_or_else(|| "pull_request.base.sha missing".to_string())?;
    let head_repo = head
        .get("repo")
        .ok_or_else(|| "pull_request.head.repo missing".to_string())?;
    let head_clone_url = head_repo
        .get("clone_url")
        .and_then(Value::as_str)
        .ok_or_else(|| "pull_request.head.repo.clone_url missing".to_string())?;

    let title = pr.get("title").and_then(Value::as_str).map(String::from);
    let html_url = pr.get("html_url").and_then(Value::as_str).map(String::from);
    let head_ref = head.get("ref").and_then(Value::as_str).map(String::from);
    let base_ref = base.get("ref").and_then(Value::as_str).map(String::from);
    let sender = payload
        .get("sender")
        .and_then(|s| s.get("login"))
        .and_then(Value::as_str)
        .map(String::from);

    Ok(PullRequestRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
        number,
        head_sha: head_sha.to_string(),
        base_sha: base_sha.to_string(),
        head_clone_url: head_clone_url.to_string(),
        head_ref,
        base_ref,
        title,
        html_url,
        sender,
    })
}

pub(crate) fn render_pr_comment_body(
    pr: &PullRequestRef,
    findings: &[Finding],
    job_id: &str,
    files_scanned: usize,
    z3_queries: u64,
    diff_only_paths: Option<usize>,
    config: &Config,
) -> String {
    let mut body = String::new();
    body.push_str(&format!(
        "**dd-formal-methods-server** — PR #{} ({}/{})\n\n",
        pr.number, pr.owner, pr.repo
    ));
    body.push_str(&format!(
        "- head: `{}`\n- base: `{}`\n- job: `{}`\n- files scanned: {}\n- Z3 queries: {}\n",
        pr.head_sha, pr.base_sha, job_id, files_scanned, z3_queries
    ));
    if let Some(n) = diff_only_paths {
        body.push_str(&format!(
            "- analysis scope: {n} changed paths (base..head diff)\n"
        ));
    } else {
        body.push_str("- analysis scope: whole tree (diff fallback)\n");
    }
    body.push_str("\n");

    if findings.is_empty() {
        body.push_str(
            "✅ No formal-methods findings. All declared `@requires` / `@ensures` / `@assert` / `@invariant` goals were discharged by Z3.\n",
        );
        return body;
    }

    let errors = findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .count();
    let warnings = findings
        .iter()
        .filter(|f| f.severity == Severity::Warning)
        .count();
    let infos = findings
        .iter()
        .filter(|f| f.severity == Severity::Info)
        .count();
    body.push_str(&format!(
        "🔎 {} finding(s): {} error · {} warning · {} info\n\n",
        findings.len(),
        errors,
        warnings,
        infos
    ));

    body.push_str("| Severity | Kind | File | Line | Message |\n");
    body.push_str("| --- | --- | --- | --- | --- |\n");
    let max_rows = config.pr_comment_max_rows;
    for f in findings.iter().take(max_rows) {
        let sev = match f.severity {
            Severity::Error => "🔴 error",
            Severity::Warning => "🟠 warning",
            Severity::Info => "🔵 info",
        };
        let kind = format!("{:?}", f.kind);
        let msg = f.message.replace('|', "\\|");
        body.push_str(&format!(
            "| {} | `{}` | `{}` | {} | {} |\n",
            sev, kind, f.file, f.line, msg
        ));
    }
    if findings.len() > max_rows {
        body.push_str(&format!(
            "\n_…and {} more finding(s); see `GET /analyses/{}` for the full list._\n",
            findings.len() - max_rows,
            job_id
        ));
    }

    let with_models: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.counterexample.as_ref().is_some_and(|m| !m.is_empty()))
        .take(5)
        .collect();
    if !with_models.is_empty() {
        body.push_str("\n<details><summary>Counterexamples</summary>\n\n");
        for f in with_models {
            body.push_str(&format!(
                "- **{}** at `{}:{}` (goal: `{}`):\n",
                format_args!("{:?}", f.kind),
                f.file,
                f.line,
                f.goal.as_deref().unwrap_or("")
            ));
            if let Some(model) = &f.counterexample {
                for (k, v) in model {
                    body.push_str(&format!("  - `{k} = {v}`\n"));
                }
            }
        }
        body.push_str("\n</details>\n");
    }

    body.push_str(
        "\n_Posted by dd-formal-methods-server. Goals come from `@requires` / `@ensures` / `@assert` / `@invariant` comments — see the project readme for the DSL._\n",
    );
    body
}

pub(crate) async fn post_pr_comment(state: &AppState, pr: &PullRequestRef, body: &str) -> Result<(), String> {
    let token = state
        .config
        .github_api_token
        .as_deref()
        .ok_or_else(|| "GITHUB_API_TOKEN is not configured".to_string())?;
    let url = format!(
        "{}/repos/{}/{}/issues/{}/comments",
        state.config.github_api_base, pr.owner, pr.repo, pr.number
    );
    let response = state
        .http
        .post(&url)
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28")
        .header(
            "user-agent",
            format!("{SERVICE_NAME}/0.1 (+https://github.com/ORESoftware/k8s-cluster)"),
        )
        .json(&json!({ "body": body }))
        .send()
        .await
        .map_err(|error| format!("failed to POST PR comment: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .unwrap_or_else(|_| "<no body>".to_string());
    if !status.is_success() {
        return Err(format!(
            "PR comment POST returned HTTP {}: {}",
            status.as_u16(),
            text.chars().take(400).collect::<String>()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_github_signature_accepts_valid_hmac() {
        let secret = "It's a Secret to Everybody";
        let body = b"Hello, World!";
        // Pre-computed HMAC-SHA256 from GitHub's docs example.
        let expected = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        assert!(verify_github_signature(secret, body, expected));
        assert!(!verify_github_signature(secret, b"tampered", expected));
        assert!(!verify_github_signature("wrong", body, expected,));
        assert!(!verify_github_signature(secret, body, "sha256=deadbeef"));
        assert!(!verify_github_signature(secret, body, "not-a-prefix"));
    }

    #[test]
    fn extract_pr_from_event_parses_minimal_payload() {
        let payload = serde_json::json!({
            "action": "opened",
            "pull_request": {
                "number": 42,
                "title": "Fix everything",
                "html_url": "https://github.com/example/repo/pull/42",
                "head": {
                    "sha": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                    "ref": "feature/x",
                    "repo": { "clone_url": "https://github.com/example/repo.git" }
                },
                "base": {
                    "sha": "cafebabecafebabecafebabecafebabecafebabe",
                    "ref": "main"
                }
            },
            "repository": { "full_name": "example/repo" },
            "sender": { "login": "octocat" }
        });
        let pr = extract_pr_from_event(&payload).unwrap();
        assert_eq!(pr.owner, "example");
        assert_eq!(pr.repo, "repo");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.head_sha.len(), 40);
        assert_eq!(pr.base_sha.len(), 40);
        assert_eq!(pr.head_clone_url, "https://github.com/example/repo.git");
        assert_eq!(pr.head_ref.as_deref(), Some("feature/x"));
        assert_eq!(pr.sender.as_deref(), Some("octocat"));
    }
}
