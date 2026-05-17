//! Thin reqwest-based GitHub API client.
//!
//! Used for three outbound operations:
//!
//! * Resolving a clone URL that embeds the install token so private repos work.
//! * Listing the files changed in a pull request, for the path-filter step.
//! * Posting a commit status back to the PR head, so the analysis result
//!   shows up as a check on the PR.
//!
//! When no token is configured we keep the client around but every outbound
//! operation degrades gracefully (logged only). This keeps local dev and
//! tests painless on public repos.

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

const APP_USER_AGENT: &str = "dd-formal-methods-service/0.1";

/// Number of attempts (including the first) made against the GitHub API
/// for idempotent reads/POSTs. Backoff: 250ms, 500ms, 1000ms.
const HTTP_RETRY_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CommitStatusState {
    Pending,
    Success,
    Failure,
    Error,
}

#[derive(Debug, Serialize)]
struct CommitStatusBody<'a> {
    state: CommitStatusState,
    target_url: Option<&'a str>,
    description: &'a str,
    context: &'a str,
}

#[derive(Debug, Deserialize)]
struct PullRequestFile {
    filename: String,
}

pub struct GithubClient {
    http: Client,
    base_url: String,
    token: Option<String>,
}

impl GithubClient {
    pub fn new(base_url: String, token: Option<String>) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static(APP_USER_AGENT));
        if let Some(tok) = &token {
            let value = format!("Bearer {tok}");
            let mut hv = HeaderValue::from_str(&value).context("invalid GITHUB_TOKEN value")?;
            hv.set_sensitive(true);
            headers.insert(AUTHORIZATION, hv);
        }

        let http = Client::builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build reqwest client")?;

        Ok(Self {
            http,
            base_url,
            token,
        })
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    /// Returns a clone URL that includes the install token (when available),
    /// suitable for `git clone`. Falls back to the public URL otherwise.
    ///
    /// IMPORTANT: the returned string contains the bearer token. Do not log
    /// it; use [`redact_url`] before any tracing call that might include it.
    pub fn authenticated_clone_url(&self, public_url: &str) -> String {
        let Some(tok) = &self.token else {
            return public_url.to_string();
        };
        if let Some(rest) = public_url.strip_prefix("https://") {
            format!("https://x-access-token:{tok}@{rest}")
        } else {
            public_url.to_string()
        }
    }

    /// Lists the file paths changed in the given PR. Up to `max_pages` pages
    /// of 100 entries each are fetched; if a `Link: rel="next"` page exists
    /// beyond that, the caller is expected to treat the PR as "in scope"
    /// (conservative default).
    ///
    /// Returns `(files, truncated)` where `truncated == true` means another
    /// page exists that we did not fetch.
    pub async fn list_pull_request_files(
        &self,
        repo_full_name: &str,
        number: u64,
        max_pages: usize,
    ) -> Result<(Vec<String>, bool)> {
        let max_pages = max_pages.max(1);
        let mut files = Vec::new();
        let mut truncated = false;

        for page in 1..=max_pages {
            let url = format!(
                "{}/repos/{}/pulls/{}/files?per_page=100&page={}",
                self.base_url.trim_end_matches('/'),
                repo_full_name,
                number,
                page
            );

            let resp = self
                .send_with_retry(reqwest::Method::GET, &url, None::<&()>)
                .await
                .with_context(|| format!("GET {url}"))?;

            let has_next = resp
                .headers()
                .get("link")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|s| s.contains("rel=\"next\""));

            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                warn!(%status, body = %body, "github list PR files returned non-success");
                return Err(anyhow::anyhow!(
                    "github list PR files failed: {status}: {body}"
                ));
            }

            let page_files: Vec<PullRequestFile> = resp
                .json()
                .await
                .with_context(|| format!("parsing PR files page {page}"))?;

            let page_len = page_files.len();
            files.extend(page_files.into_iter().map(|f| f.filename));

            // If GitHub gave us a short page or no next link, we're done.
            if !has_next || page_len < 100 {
                truncated = false;
                break;
            }
            // Reached the cap and there's more.
            if page == max_pages && has_next {
                truncated = true;
            }
        }

        debug!(
            repo = repo_full_name,
            pr = number,
            files = files.len(),
            truncated,
            "fetched PR files"
        );
        Ok((files, truncated))
    }

    /// Posts a commit status to the PR head. No-op when the client has no
    /// token configured. Retries on transient 5xx / connection errors.
    pub async fn post_commit_status(
        &self,
        repo_full_name: &str,
        sha: &str,
        state: CommitStatusState,
        context: &str,
        description: &str,
        target_url: Option<&str>,
    ) -> Result<()> {
        if self.token.is_none() {
            info!(
                repo = repo_full_name,
                sha,
                ?state,
                context,
                description,
                "GITHUB_TOKEN not configured; skipping commit status"
            );
            return Ok(());
        }

        let url = format!(
            "{}/repos/{}/statuses/{}",
            self.base_url.trim_end_matches('/'),
            repo_full_name,
            sha
        );

        let body = CommitStatusBody {
            state,
            target_url,
            description: truncate_description(description),
            context,
        };

        let resp = self
            .send_with_retry(reqwest::Method::POST, &url, Some(&body))
            .await
            .with_context(|| format!("POST {url}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_txt = resp.text().await.unwrap_or_default();
            warn!(%status, body = %body_txt, "github commit status returned non-success");
            return Err(anyhow::anyhow!(
                "github commit status failed: {status}: {body_txt}"
            ));
        }
        Ok(())
    }

    /// Sends a request, retrying once on transient 5xx and connection errors.
    /// Uses a short exponential backoff bounded by 1s to keep webhooks snappy.
    async fn send_with_retry<B: Serialize + ?Sized>(
        &self,
        method: reqwest::Method,
        url: &str,
        json_body: Option<&B>,
    ) -> Result<reqwest::Response> {
        let mut delay = Duration::from_millis(250);
        let mut last_err: Option<anyhow::Error> = None;

        for attempt in 1..=HTTP_RETRY_ATTEMPTS {
            let mut req = self.http.request(method.clone(), url);
            if let Some(body) = json_body {
                req = req.json(body);
            }

            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
                        warn!(%status, attempt, "github transient error; retrying");
                        last_err = Some(anyhow::anyhow!("transient HTTP status {status}"));
                    } else {
                        return Ok(resp);
                    }
                }
                Err(e) if e.is_timeout() || e.is_connect() => {
                    warn!(error = %e, attempt, "github transport error; retrying");
                    last_err = Some(anyhow::anyhow!("transport: {e}"));
                }
                Err(e) => return Err(anyhow::anyhow!("github request: {e}")),
            }

            if attempt < HTTP_RETRY_ATTEMPTS {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(1));
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("github retry budget exhausted")))
    }
}

/// Commit status descriptions are capped at 140 chars by GitHub. Truncate
/// long lines defensively.
fn truncate_description(s: &str) -> &str {
    if s.len() <= 140 {
        s
    } else {
        // Find the largest valid UTF-8 boundary ≤ 140.
        let mut end = 140;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

/// Removes any embedded `x-access-token:...@` segment from a URL-shaped
/// string, replacing the secret with `***`. Used before logging anything
/// that may have been built from [`GithubClient::authenticated_clone_url`].
pub fn redact_url(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(idx) = rest.find("x-access-token:") {
        out.push_str(&rest[..idx]);
        out.push_str("x-access-token:***");
        rest = &rest[idx + "x-access-token:".len()..];
        if let Some(at) = rest.find('@') {
            rest = &rest[at..];
        } else {
            break;
        }
    }
    out.push_str(rest);
    out
}

impl std::fmt::Debug for GithubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubClient")
            .field("base_url", &self.base_url)
            .field("has_token", &self.token.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_url_embeds_token() {
        let client =
            GithubClient::new("https://api.github.com".into(), Some("ghp_abc".into())).unwrap();
        let url = client.authenticated_clone_url("https://github.com/owner/repo.git");
        assert_eq!(
            url,
            "https://x-access-token:ghp_abc@github.com/owner/repo.git"
        );
    }

    #[test]
    fn authenticated_url_passes_through_without_token() {
        let client = GithubClient::new("https://api.github.com".into(), None).unwrap();
        let url = client.authenticated_clone_url("https://github.com/owner/repo.git");
        assert_eq!(url, "https://github.com/owner/repo.git");
    }

    #[test]
    fn redact_url_replaces_token_segment() {
        let url = "https://x-access-token:ghp_super_secret@github.com/owner/repo.git";
        assert_eq!(
            redact_url(url),
            "https://x-access-token:***@github.com/owner/repo.git"
        );
    }

    #[test]
    fn redact_url_passes_through_when_no_token() {
        let url = "https://github.com/owner/repo.git";
        assert_eq!(redact_url(url), url);
    }

    #[test]
    fn redact_url_handles_multiple_occurrences() {
        let url = "a x-access-token:tok1@b x-access-token:tok2@c";
        assert_eq!(
            redact_url(url),
            "a x-access-token:***@b x-access-token:***@c"
        );
    }

    #[test]
    fn truncates_description_safely_on_char_boundary() {
        let s: String = "x".repeat(200);
        assert_eq!(truncate_description(&s).len(), 140);
        // Description ending with a multi-byte glyph: must not split mid-codepoint.
        let mut multi = "x".repeat(139);
        multi.push('€'); // 3-byte UTF-8 char
        multi.push_str(&"y".repeat(20));
        let truncated = truncate_description(&multi);
        assert!(truncated.is_char_boundary(truncated.len()));
        assert!(truncated.len() <= 140);
    }
}
