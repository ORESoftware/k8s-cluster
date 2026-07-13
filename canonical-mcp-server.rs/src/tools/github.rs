//! GitHub API access and pure response summarization for the
//! canonical-cloud organization.
//!
//! Network access is confined to [`GitHubClient`] and the `*_report`
//! orchestration functions; everything that interprets response JSON is a
//! pure function over `serde_json::Value` so tests can feed fixtures.

use serde::Serialize;
use serde_json::Value;

/// GitHub organization that owns the canonical.cloud stack.
pub const ORG: &str = "canonical-cloud";

/// Repositories summarized by `stack_ci_status`.
pub const STACK_REPOS: [&str; 4] = [
    "canonical-monorepo",
    "canonical-web-server.rs",
    "canonical-marketing-site.web",
    "canonical-interfaces",
];

/// Monorepo that pins the app repositories as git submodules.
pub const MONOREPO: &str = "canonical-monorepo";

const API_ROOT: &str = "https://api.github.com";

/// GitHub requires a User-Agent on every API request.
pub const USER_AGENT: &str = concat!("canonical-mcp-server/", env!("CARGO_PKG_VERSION"));

/// Thin GitHub REST client. Unauthenticated by default; sends a Bearer
/// token when `GITHUB_TOKEN` or `GH_TOKEN` is set.
pub struct GitHubClient {
    http: reqwest::Client,
    token: Option<String>,
}

impl GitHubClient {
    pub fn new(http: reqwest::Client) -> Self {
        let token = std::env::var("GITHUB_TOKEN")
            .or_else(|_| std::env::var("GH_TOKEN"))
            .ok()
            .filter(|token| !token.trim().is_empty());
        Self { http, token }
    }

    fn request(&self, url: &str, accept: &'static str) -> reqwest::RequestBuilder {
        let mut request = self
            .http
            .get(url)
            .header("Accept", accept)
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        request
    }

    async fn get(&self, url: &str, accept: &'static str) -> Result<String, String> {
        let response = self
            .request(url, accept)
            .send()
            .await
            .map_err(|error| format!("GET {url} failed: {}", super::error_chain(&error)))?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            format!(
                "GET {url}: error reading body: {}",
                super::error_chain(&error)
            )
        })?;
        if !status.is_success() {
            let snippet: String = body.chars().take(300).collect();
            return Err(format!("GET {url} returned {status}: {snippet}"));
        }
        Ok(body)
    }

    /// GET a GitHub API path (relative to the API root) as JSON.
    pub async fn get_json(&self, path: &str) -> Result<Value, String> {
        let url = format!("{API_ROOT}{path}");
        let body = self.get(&url, "application/vnd.github+json").await?;
        serde_json::from_str(&body).map_err(|error| format!("GET {url}: invalid JSON: {error}"))
    }

    /// GET a repository file's raw contents through the contents API.
    pub async fn get_raw_contents(&self, path: &str) -> Result<String, String> {
        let url = format!("{API_ROOT}{path}");
        self.get(&url, "application/vnd.github.raw+json").await
    }
}

/// Compact summary of one GitHub Actions workflow run.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RunSummary {
    pub repo: String,
    pub workflow: String,
    pub branch: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub url: String,
    pub updated_at: String,
}

fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Summarize a `GET /repos/{org}/{repo}/actions/runs` response body.
pub fn summarize_runs(repo: &str, body: &Value) -> Vec<RunSummary> {
    let Some(runs) = body.get("workflow_runs").and_then(Value::as_array) else {
        return Vec::new();
    };
    runs.iter()
        .map(|run| RunSummary {
            repo: repo.to_string(),
            workflow: str_field(run, "name"),
            branch: str_field(run, "head_branch"),
            status: str_field(run, "status"),
            conclusion: run
                .get("conclusion")
                .and_then(Value::as_str)
                .map(str::to_string),
            url: str_field(run, "html_url"),
            updated_at: str_field(run, "updated_at"),
        })
        .collect()
}

/// One `[submodule "..."]` section from a `.gitmodules` file.
#[derive(Debug, PartialEq, Eq)]
pub struct GitmoduleEntry {
    pub path: String,
    pub url: String,
}

/// Parse the `path`/`url` pairs out of a `.gitmodules` file.
pub fn parse_gitmodules(contents: &str) -> Vec<GitmoduleEntry> {
    let mut entries = Vec::new();
    let mut path: Option<String> = None;
    let mut url: Option<String> = None;
    let mut flush = |path: &mut Option<String>, url: &mut Option<String>| {
        if let (Some(path), Some(url)) = (path.take(), url.take()) {
            entries.push(GitmoduleEntry { path, url });
        }
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            flush(&mut path, &mut url);
        } else if let Some((key, value)) = line.split_once('=') {
            match key.trim() {
                "path" => path = Some(value.trim().to_string()),
                "url" => url = Some(value.trim().to_string()),
                _ => {}
            }
        }
    }
    flush(&mut path, &mut url);
    entries
}

/// Extract the repository name from a submodule URL, accepting both
/// `git@github.com:org/repo(.git)` and `https://github.com/org/repo(.git)`.
pub fn repo_from_git_url(url: &str) -> Option<String> {
    let tail = url.rsplit(['/', ':']).next()?;
    let repo = tail.strip_suffix(".git").unwrap_or(tail);
    if repo.is_empty() {
        None
    } else {
        Some(repo.to_string())
    }
}

/// Map submodule directory-listing entries (`GET .../contents/apps`) to
/// `(name, pinned_sha)` pairs. Gitlink entries appear as zero-size files
/// with no `download_url` (older API shapes report `type: "submodule"`).
pub fn pins_from_listing(body: &Value) -> Vec<(String, String)> {
    let Some(entries) = body.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter(|entry| {
            let kind = entry.get("type").and_then(Value::as_str).unwrap_or("");
            let size = entry.get("size").and_then(Value::as_u64);
            let no_download = entry
                .get("download_url")
                .map(Value::is_null)
                .unwrap_or(true);
            kind == "submodule" || (kind == "file" && size == Some(0) && no_download)
        })
        .filter_map(|entry| {
            let name = entry.get("name").and_then(Value::as_str)?;
            let sha = entry.get("sha").and_then(Value::as_str)?;
            Some((name.to_string(), sha.to_string()))
        })
        .collect()
}

/// Extract the commit SHA from a `GET /repos/{org}/{repo}/commits/{ref}`
/// response body.
pub fn commit_sha(body: &Value) -> Option<String> {
    body.get("sha").and_then(Value::as_str).map(str::to_string)
}

/// Extract `ahead_by` from a `GET /repos/{org}/{repo}/compare/{a}...{b}`
/// response body.
pub fn compare_ahead_by(body: &Value) -> Option<u64> {
    body.get("ahead_by").and_then(Value::as_u64)
}

/// Pin status of one monorepo app submodule versus its repo's `main`.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct PinStatus {
    pub app: String,
    pub repo: String,
    pub pinned_sha: String,
    pub main_head_sha: Option<String>,
    pub current: Option<bool>,
    pub behind_by: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Combine a pin SHA with the app repo's `main` HEAD into a report row.
pub fn build_pin_status(
    app: &str,
    repo: &str,
    pinned_sha: &str,
    main_head_sha: Option<String>,
    behind_by: Option<u64>,
    error: Option<String>,
) -> PinStatus {
    let current = main_head_sha.as_deref().map(|head| head == pinned_sha);
    PinStatus {
        app: app.to_string(),
        repo: repo.to_string(),
        pinned_sha: pinned_sha.to_string(),
        main_head_sha,
        current,
        behind_by,
        error,
    }
}

/// `stack_ci_status`: latest workflow runs for each stack repository.
pub async fn ci_status_report(
    client: &GitHubClient,
    repo_filter: Option<&str>,
) -> Result<Value, String> {
    let repos: Vec<&str> = match repo_filter {
        Some(repo) => {
            if !STACK_REPOS.contains(&repo) {
                return Err(format!(
                    "unknown repo {repo:?}; expected one of {STACK_REPOS:?}"
                ));
            }
            vec![repo]
        }
        None => STACK_REPOS.to_vec(),
    };
    let mut report = Vec::new();
    for repo in repos {
        let path = format!("/repos/{ORG}/{repo}/actions/runs?per_page=5");
        match client.get_json(&path).await {
            Ok(body) => report.push(serde_json::json!({
                "repo": repo,
                "runs": summarize_runs(repo, &body),
            })),
            Err(error) => report.push(serde_json::json!({
                "repo": repo,
                "error": error,
            })),
        }
    }
    Ok(Value::Array(report))
}

/// `submodule_pins`: compare each monorepo app pin against the app repo's
/// `main` HEAD.
pub async fn submodule_pins_report(client: &GitHubClient) -> Result<Value, String> {
    let gitmodules = client
        .get_raw_contents(&format!(
            "/repos/{ORG}/{MONOREPO}/contents/.gitmodules?ref=main"
        ))
        .await?;
    let modules = parse_gitmodules(&gitmodules);

    let listing = client
        .get_json(&format!("/repos/{ORG}/{MONOREPO}/contents/apps?ref=main"))
        .await?;
    let pins = pins_from_listing(&listing);

    let mut report = Vec::new();
    for (name, pinned_sha) in pins {
        let app_path = format!("apps/{name}");
        let repo = modules
            .iter()
            .find(|module| module.path == app_path)
            .and_then(|module| repo_from_git_url(&module.url))
            .unwrap_or_else(|| name.clone());

        let head = client
            .get_json(&format!("/repos/{ORG}/{repo}/commits/main"))
            .await;
        let status = match head {
            Ok(body) => {
                let main_head_sha = commit_sha(&body);
                let behind_by = if main_head_sha.as_deref() == Some(pinned_sha.as_str()) {
                    Some(0)
                } else {
                    let compare = client
                        .get_json(&format!("/repos/{ORG}/{repo}/compare/{pinned_sha}...main"))
                        .await;
                    compare.ok().as_ref().and_then(compare_ahead_by)
                };
                build_pin_status(&name, &repo, &pinned_sha, main_head_sha, behind_by, None)
            }
            Err(error) => build_pin_status(&name, &repo, &pinned_sha, None, None, Some(error)),
        };
        report.push(status);
    }
    serde_json::to_value(report).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summarize_runs_extracts_compact_fields() {
        let body = json!({
            "total_count": 2,
            "workflow_runs": [
                {
                    "name": "ci",
                    "head_branch": "main",
                    "status": "completed",
                    "conclusion": "success",
                    "html_url": "https://github.com/canonical-cloud/x/actions/runs/1",
                    "updated_at": "2026-07-13T17:23:58Z"
                },
                {
                    "name": "ci",
                    "head_branch": "feature",
                    "status": "in_progress",
                    "conclusion": null,
                    "html_url": "https://github.com/canonical-cloud/x/actions/runs/2",
                    "updated_at": "2026-07-13T18:00:00Z"
                }
            ]
        });
        let runs = summarize_runs("x", &body);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].repo, "x");
        assert_eq!(runs[0].workflow, "ci");
        assert_eq!(runs[0].branch, "main");
        assert_eq!(runs[0].conclusion.as_deref(), Some("success"));
        assert_eq!(runs[1].status, "in_progress");
        assert_eq!(runs[1].conclusion, None);
    }

    #[test]
    fn summarize_runs_tolerates_missing_list() {
        assert!(summarize_runs("x", &json!({"message": "Not Found"})).is_empty());
    }

    #[test]
    fn parse_gitmodules_reads_all_sections() {
        let contents = "\
[submodule \"apps/canonical-web-server.rs\"]\n\
\tpath = apps/canonical-web-server.rs\n\
\turl = git@github.com:canonical-cloud/canonical-web-server.rs\n\
\tbranch = main\n\
[submodule \"apps/canonical-interfaces\"]\n\
\tpath = apps/canonical-interfaces\n\
\turl = https://github.com/canonical-cloud/canonical-interfaces.git\n";
        let entries = parse_gitmodules(contents);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "apps/canonical-web-server.rs");
        assert_eq!(
            entries[0].url,
            "git@github.com:canonical-cloud/canonical-web-server.rs"
        );
        assert_eq!(entries[1].path, "apps/canonical-interfaces");
    }

    #[test]
    fn repo_from_git_url_handles_ssh_and_https() {
        assert_eq!(
            repo_from_git_url("git@github.com:canonical-cloud/canonical-web-server.rs").as_deref(),
            Some("canonical-web-server.rs")
        );
        assert_eq!(
            repo_from_git_url("https://github.com/canonical-cloud/canonical-interfaces.git")
                .as_deref(),
            Some("canonical-interfaces")
        );
        assert_eq!(repo_from_git_url(""), None);
    }

    #[test]
    fn pins_from_listing_selects_gitlink_entries() {
        let body = json!([
            {
                "name": "canonical-interfaces",
                "sha": "8cef0172",
                "size": 0,
                "type": "file",
                "download_url": null
            },
            {
                "name": "explicit-submodule",
                "sha": "aabbcc",
                "size": 0,
                "type": "submodule",
                "download_url": null
            },
            {
                "name": "README.md",
                "sha": "ffee00",
                "size": 120,
                "type": "file",
                "download_url": "https://raw.githubusercontent.com/x/y/main/apps/README.md"
            },
            {
                "name": "subdir",
                "sha": "112233",
                "size": 0,
                "type": "dir",
                "download_url": null
            }
        ]);
        let pins = pins_from_listing(&body);
        assert_eq!(
            pins,
            vec![
                ("canonical-interfaces".to_string(), "8cef0172".to_string()),
                ("explicit-submodule".to_string(), "aabbcc".to_string()),
            ]
        );
    }

    #[test]
    fn commit_and_compare_extraction() {
        assert_eq!(
            commit_sha(&json!({"sha": "abc123"})).as_deref(),
            Some("abc123")
        );
        assert_eq!(commit_sha(&json!({"message": "Not Found"})), None);
        assert_eq!(compare_ahead_by(&json!({"ahead_by": 3})), Some(3));
        assert_eq!(compare_ahead_by(&json!({})), None);
    }

    #[test]
    fn build_pin_status_flags_current_and_stale_pins() {
        let current = build_pin_status("a", "a", "abc", Some("abc".to_string()), Some(0), None);
        assert_eq!(current.current, Some(true));
        assert_eq!(current.behind_by, Some(0));

        let stale = build_pin_status("a", "a", "abc", Some("def".to_string()), Some(4), None);
        assert_eq!(stale.current, Some(false));
        assert_eq!(stale.behind_by, Some(4));

        let unknown = build_pin_status("a", "a", "abc", None, None, Some("boom".to_string()));
        assert_eq!(unknown.current, None);
        assert_eq!(unknown.error.as_deref(), Some("boom"));
    }
}
