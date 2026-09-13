use std::{collections::BTreeMap, sync::Arc};

use axum::http::StatusCode;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Notify};

#[derive(Clone, Debug)]
pub struct HttpResult {
    pub status: StatusCode,
    pub body: Value,
}

impl HttpResult {
    pub fn new(status: StatusCode, body: Value) -> Self {
        Self { status, body }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetentionKind {
    None,
    Accepted,
    Ambiguous,
}

#[derive(Clone, Debug)]
pub struct SubmissionOutcome {
    pub response: HttpResult,
    pub retention: RetentionKind,
}

impl SubmissionOutcome {
    pub fn transient(status: StatusCode, body: Value) -> Self {
        Self {
            response: HttpResult::new(status, body),
            retention: RetentionKind::None,
        }
    }

    pub fn accepted(body: Value) -> Self {
        Self {
            response: HttpResult::new(StatusCode::ACCEPTED, body),
            retention: RetentionKind::Accepted,
        }
    }

    pub fn ambiguous(body: Value) -> Self {
        Self {
            response: HttpResult::new(StatusCode::BAD_GATEWAY, body),
            retention: RetentionKind::Ambiguous,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExecutorAssignment {
    pub id: String,
    pub provider: String,
}

#[derive(Clone, Debug)]
struct RetainedAssignment {
    request_hash: [u8; 32],
    result: HttpResult,
    kind: RetentionKind,
    executor: ExecutorAssignment,
}

pub struct InflightClaim {
    request_hash: [u8; 32],
    selection: Mutex<Option<ExecutorAssignment>>,
    result: Mutex<Option<HttpResult>>,
    notify: Notify,
}

impl InflightClaim {
    fn new(request_hash: [u8; 32]) -> Self {
        Self {
            request_hash,
            selection: Mutex::new(None),
            result: Mutex::new(None),
            notify: Notify::new(),
        }
    }

    pub async fn select_executor(&self, id: &str, provider: &str) -> Result<(), &'static str> {
        let mut selection = self.selection.lock().await;
        if selection.is_some() {
            return Err("executor assignment is already selected");
        }
        *selection = Some(ExecutorAssignment {
            id: id.to_string(),
            provider: provider.to_string(),
        });
        Ok(())
    }

    pub async fn wait(&self) -> HttpResult {
        loop {
            let notified = self.notify.notified();
            if let Some(result) = self.result.lock().await.clone() {
                return result;
            }
            notified.await;
        }
    }
}

pub enum BeginClaim {
    Owner(Arc<InflightClaim>),
    Wait(Arc<InflightClaim>),
    Retained(HttpResult),
    Conflict,
    Capacity,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AssignmentCounts {
    pub accepted: usize,
    pub ambiguous: usize,
    pub inflight: usize,
    pub accepted_aws: usize,
    pub accepted_hetzner: usize,
    pub ambiguous_aws: usize,
    pub ambiguous_hetzner: usize,
}

#[derive(Default)]
struct RegistryState {
    retained: BTreeMap<String, RetainedAssignment>,
    inflight: BTreeMap<String, Arc<InflightClaim>>,
}

#[derive(Clone)]
pub struct AssignmentRegistry {
    state: Arc<Mutex<RegistryState>>,
    max_entries: usize,
}

impl AssignmentRegistry {
    pub fn new(max_entries: usize) -> Result<Self, &'static str> {
        if max_entries == 0 {
            return Err("assignment capacity must be positive");
        }
        Ok(Self {
            state: Arc::new(Mutex::new(RegistryState::default())),
            max_entries,
        })
    }

    pub async fn begin(&self, request_id: &str, request_hash: [u8; 32]) -> BeginClaim {
        let mut state = self.state.lock().await;
        if let Some(retained) = state.retained.get(request_id) {
            return if retained.request_hash == request_hash {
                BeginClaim::Retained(retained.result.clone())
            } else {
                BeginClaim::Conflict
            };
        }
        if let Some(inflight) = state.inflight.get(request_id) {
            return if inflight.request_hash == request_hash {
                BeginClaim::Wait(inflight.clone())
            } else {
                BeginClaim::Conflict
            };
        }
        if state.retained.len().saturating_add(state.inflight.len()) >= self.max_entries {
            return BeginClaim::Capacity;
        }
        let claim = Arc::new(InflightClaim::new(request_hash));
        state
            .inflight
            .insert(request_id.to_string(), claim.clone());
        BeginClaim::Owner(claim)
    }

    pub async fn complete(
        &self,
        request_id: &str,
        claim: &Arc<InflightClaim>,
        outcome: SubmissionOutcome,
    ) {
        let selection = claim.selection.lock().await.clone();
        {
            let mut state = self.state.lock().await;
            let is_owner = state
                .inflight
                .get(request_id)
                .is_some_and(|current| Arc::ptr_eq(current, claim));
            if is_owner {
                state.inflight.remove(request_id);
                if outcome.retention != RetentionKind::None {
                    let executor = selection.unwrap_or_else(|| ExecutorAssignment {
                        id: "unassigned".to_string(),
                        provider: "unknown".to_string(),
                    });
                    state.retained.insert(
                        request_id.to_string(),
                        RetainedAssignment {
                            request_hash: claim.request_hash,
                            result: outcome.response.clone(),
                            kind: outcome.retention,
                            executor,
                        },
                    );
                }
            }
        }
        *claim.result.lock().await = Some(outcome.response);
        claim.notify.notify_waiters();
    }

    pub async fn counts(&self) -> AssignmentCounts {
        let state = self.state.lock().await;
        let mut counts = AssignmentCounts {
            inflight: state.inflight.len(),
            ..AssignmentCounts::default()
        };
        for retained in state.retained.values() {
            let provider = retained.executor.provider.as_str();
            let _executor_id = retained.executor.id.as_str();
            match retained.kind {
                RetentionKind::Accepted => {
                    counts.accepted += 1;
                    if provider == "aws" {
                        counts.accepted_aws += 1;
                    } else if provider == "hetzner" {
                        counts.accepted_hetzner += 1;
                    }
                }
                RetentionKind::Ambiguous => {
                    counts.ambiguous += 1;
                    if provider == "aws" {
                        counts.ambiguous_aws += 1;
                    } else if provider == "hetzner" {
                        counts.ambiguous_hetzner += 1;
                    }
                }
                RetentionKind::None => {}
            }
        }
        counts
    }
}

pub fn request_fingerprint(
    request_id: &str,
    repository: &str,
    revision: &str,
    profile: &str,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    for value in [
        "build-server.v1",
        "run-profile",
        request_id,
        repository,
        revision,
        profile,
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    digest.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hash(profile: &str) -> [u8; 32] {
        request_fingerprint(
            "request-one",
            "ORESoftware/k8s-cluster",
            "0123456789abcdef0123456789abcdef01234567",
            profile,
        )
    }

    #[tokio::test]
    async fn retained_results_are_content_bound() {
        let registry = AssignmentRegistry::new(4).unwrap();
        let BeginClaim::Owner(claim) = registry.begin("request-one", hash("rust")).await else {
            panic!("first request must own its claim");
        };
        claim.select_executor("aws-primary", "aws").await.unwrap();
        registry
            .complete(
                "request-one",
                &claim,
                SubmissionOutcome::accepted(json!({"id": "aws-primary~job"})),
            )
            .await;

        assert!(matches!(
            registry.begin("request-one", hash("rust")).await,
            BeginClaim::Retained(_)
        ));
        assert!(matches!(
            registry.begin("request-one", hash("node")).await,
            BeginClaim::Conflict
        ));
        assert_eq!(registry.counts().await.accepted_aws, 1);
    }

    #[tokio::test]
    async fn exact_inflight_duplicates_wait_and_conflicts_fail_closed() {
        let registry = AssignmentRegistry::new(4).unwrap();
        let BeginClaim::Owner(owner) = registry.begin("request-one", hash("rust")).await else {
            panic!("first request must own its claim");
        };
        let BeginClaim::Wait(waiter) = registry.begin("request-one", hash("rust")).await else {
            panic!("exact duplicate must wait");
        };
        assert!(Arc::ptr_eq(&owner, &waiter));
        assert!(matches!(
            registry.begin("request-one", hash("node")).await,
            BeginClaim::Conflict
        ));
    }

    #[tokio::test]
    async fn capacity_fails_closed_without_evicting_assignments() {
        let registry = AssignmentRegistry::new(1).unwrap();
        assert!(matches!(
            registry.begin("request-one", hash("rust")).await,
            BeginClaim::Owner(_)
        ));
        assert!(matches!(
            registry.begin("request-two", hash("rust")).await,
            BeginClaim::Capacity
        ));
    }
}
