//! Shared application state, wired into every axum handler.

use std::sync::{Arc, Mutex};

use tokio::sync::Semaphore;

use crate::analysis::pipeline::Pipeline;
use crate::config::Config;
use crate::dedupe::DeliveryDedupe;
use crate::github::GithubClient;
use crate::path_filter::PathFilter;
use crate::repo_allowlist::RepoAllowlist;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub github: Arc<GithubClient>,
    pub pipeline: Arc<Pipeline>,

    /// Bounds the number of analyses in flight to keep CPU/disk load
    /// predictable. Handlers acquire a permit before dispatching work.
    pub analysis_semaphore: Arc<Semaphore>,

    /// Allowlist of `owner/repo` slugs this service will analyze.
    pub repo_allowlist: Arc<RepoAllowlist>,

    /// Path-prefix filter on the PR's changed-file list.
    pub path_filter: Arc<PathFilter>,

    /// Dedupes `X-GitHub-Delivery` IDs across short windows so that
    /// GitHub's retry attempts on a single delivery do not kick off
    /// duplicate analyses.
    pub delivery_dedupe: Arc<Mutex<DeliveryDedupe>>,
}
