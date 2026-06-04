//! Subset of GitHub webhook payload types we care about.
//!
//! These are deliberately partial: we deserialise only the fields the
//! analyzer needs (repo + head SHA + PR coordinates), so payload shape
//! changes in unrelated parts of the GitHub schema don't break parsing.

use serde::Deserialize;

/// Action on a `pull_request` event. We treat anything outside of the
/// "code changed" subset as a no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestAction {
    Opened,
    Reopened,
    Synchronize,
    ReadyForReview,
    Edited,
    Closed,
    Assigned,
    Unassigned,
    Labeled,
    Unlabeled,
    ReviewRequested,
    ReviewRequestRemoved,
    ConvertedToDraft,
    Locked,
    Unlocked,
    Milestoned,
    Demilestoned,
    AutoMergeEnabled,
    AutoMergeDisabled,
    Dequeued,
    Enqueued,
    #[serde(other)]
    Other,
}

impl PullRequestAction {
    /// Action types that should trigger a fresh analysis run.
    pub fn should_analyze(self) -> bool {
        matches!(
            self,
            PullRequestAction::Opened
                | PullRequestAction::Reopened
                | PullRequestAction::Synchronize
                | PullRequestAction::ReadyForReview
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PullRequestEvent {
    pub action: PullRequestAction,
    pub number: u64,
    pub pull_request: PullRequest,
    pub repository: Repository,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PullRequest {
    pub id: u64,
    pub number: u64,
    pub state: String,
    pub draft: Option<bool>,
    pub title: String,
    pub head: GitRef,
    pub base: GitRef,
    #[serde(default)]
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitRef {
    /// Branch name.
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub sha: String,
    pub repo: Option<Repository>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Repository {
    pub id: u64,
    pub name: String,
    pub full_name: String,
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub clone_url: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    pub owner: RepoOwner,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepoOwner {
    pub login: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_opened_event() {
        let payload = serde_json::json!({
            "action": "opened",
            "number": 42,
            "pull_request": {
                "id": 1,
                "number": 42,
                "state": "open",
                "draft": false,
                "title": "test",
                "head": { "ref": "feature", "sha": "deadbeef", "repo": null },
                "base": { "ref": "staging", "sha": "cafef00d", "repo": null },
                "html_url": "https://example.invalid/pr/42",
            },
            "repository": {
                "id": 100,
                "name": "repo",
                "full_name": "owner/repo",
                "private": true,
                "clone_url": "https://github.com/owner/repo.git",
                "html_url": "https://github.com/owner/repo",
                "owner": { "login": "owner" }
            }
        });
        let event: PullRequestEvent = serde_json::from_value(payload).expect("parse");
        assert_eq!(event.action, PullRequestAction::Opened);
        assert!(event.action.should_analyze());
        assert_eq!(event.number, 42);
        assert_eq!(event.pull_request.head.sha, "deadbeef");
        assert_eq!(event.repository.full_name, "owner/repo");
    }

    #[test]
    fn unknown_action_maps_to_other() {
        let payload = serde_json::json!({
            "action": "spaceship_launched",
            "number": 1,
            "pull_request": {
                "id": 1, "number": 1, "state": "open", "draft": false, "title": "t",
                "head": { "ref": "x", "sha": "y", "repo": null },
                "base": { "ref": "x", "sha": "y", "repo": null }
            },
            "repository": {
                "id": 1, "name": "r", "full_name": "o/r", "private": false,
                "owner": { "login": "o" }
            }
        });
        let event: PullRequestEvent = serde_json::from_value(payload).expect("parse");
        assert_eq!(event.action, PullRequestAction::Other);
        assert!(!event.action.should_analyze());
    }
}
