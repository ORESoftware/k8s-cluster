//! `stack_docs`: fetch operational docs from the canonical-monorepo.

use serde::Deserialize;

const RAW_ROOT: &str = "https://raw.githubusercontent.com/canonical-cloud/canonical-monorepo/main";

/// The documents `stack_docs` can fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DocName {
    /// docs/deploy.md — how the stack is deployed.
    Deploy,
    /// docs/repo-boundaries.md — which repo owns what.
    RepoBoundaries,
}

impl DocName {
    pub fn file_name(self) -> &'static str {
        match self {
            DocName::Deploy => "deploy.md",
            DocName::RepoBoundaries => "repo-boundaries.md",
        }
    }

    pub fn url(self) -> String {
        format!("{RAW_ROOT}/docs/{}", self.file_name())
    }
}

/// Fetch the raw markdown for `doc`.
pub async fn fetch(client: &reqwest::Client, doc: DocName) -> Result<String, String> {
    let url = doc.url();
    let response = client
        .get(&url)
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
        return Err(format!("GET {url} returned {status}"));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_urls_point_at_monorepo_docs() {
        assert_eq!(
            DocName::Deploy.url(),
            "https://raw.githubusercontent.com/canonical-cloud/canonical-monorepo/main/docs/deploy.md"
        );
        assert_eq!(
            DocName::RepoBoundaries.url(),
            "https://raw.githubusercontent.com/canonical-cloud/canonical-monorepo/main/docs/repo-boundaries.md"
        );
    }

    #[test]
    fn doc_name_deserializes_kebab_case() {
        assert_eq!(
            serde_json::from_str::<DocName>("\"deploy\"").unwrap(),
            DocName::Deploy
        );
        assert_eq!(
            serde_json::from_str::<DocName>("\"repo-boundaries\"").unwrap(),
            DocName::RepoBoundaries
        );
        assert!(serde_json::from_str::<DocName>("\"nope\"").is_err());
    }
}
