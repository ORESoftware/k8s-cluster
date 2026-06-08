use std::{
    collections::BTreeMap,
    sync::{atomic::AtomicU64, Arc},
};

use tokio::sync::RwLock;

use crate::{
    audit::run_audit,
    config::Config,
    metrics::Metrics,
    models::{AuditRequest, JobRecord, JobStatus},
    observability::{log_error, log_info},
    util::{next_id, now_ms},
};

pub struct JobStore {
    jobs: RwLock<BTreeMap<String, JobRecord>>,
    counter: AtomicU64,
    max_jobs: usize,
}

impl JobStore {
    pub fn new(max_jobs: usize) -> Self {
        Self {
            jobs: RwLock::new(BTreeMap::new()),
            counter: AtomicU64::new(0),
            max_jobs,
        }
    }

    pub async fn enqueue(
        self: Arc<Self>,
        config: Arc<Config>,
        http: reqwest::Client,
        metrics: Arc<Metrics>,
        request: AuditRequest,
    ) -> JobRecord {
        let id = next_id("audit", &self.counter);
        let request_id = request
            .request_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| id.clone());
        let record = JobRecord {
            id: id.clone(),
            request_id,
            status: JobStatus::Queued,
            created_at_ms: now_ms(),
            started_at_ms: None,
            finished_at_ms: None,
            result: None,
            error: None,
        };
        {
            let mut jobs = self.jobs.write().await;
            if jobs.len() >= self.max_jobs {
                if let Some(oldest_id) = jobs
                    .iter()
                    .min_by_key(|(_, record)| record.created_at_ms)
                    .map(|(id, _)| id.clone())
                {
                    jobs.remove(&oldest_id);
                }
            }
            jobs.insert(id.clone(), record.clone());
        }
        metrics.audit_submitted_total.fetch_add(1);
        log_info(
            "compliance.audit.queued",
            "compliance audit queued",
            serde_json::json!({ "jobId": id, "targetKind": format!("{:?}", request.target.kind) }),
        );

        let store = self.clone();
        tokio::spawn(async move {
            store.mark_running(&id).await;
            match run_audit(config, http, request, id.clone()).await {
                Ok(report) => {
                    metrics.audit_completed_total.fetch_add(1);
                    metrics
                        .standards_evaluated_total
                        .fetch_add(report.standard_results.len() as u64);
                    metrics
                        .findings_total
                        .fetch_add(report.findings.len() as u64);
                    store.mark_succeeded(&id, report).await;
                    log_info(
                        "compliance.audit.completed",
                        "compliance audit completed",
                        serde_json::json!({ "jobId": id }),
                    );
                }
                Err(error) => {
                    metrics.audit_failed_total.fetch_add(1);
                    metrics.errors_total.fetch_add(1);
                    store.mark_failed(&id, error.clone()).await;
                    log_error(
                        "compliance.audit.failed",
                        "compliance audit failed",
                        serde_json::json!({ "jobId": id, "error": error }),
                    );
                }
            }
        });
        record
    }

    pub async fn list(&self) -> Vec<JobRecord> {
        let mut jobs = self.jobs.read().await.values().cloned().collect::<Vec<_>>();
        jobs.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
        jobs
    }

    pub async fn get(&self, id: &str) -> Option<JobRecord> {
        self.jobs.read().await.get(id).cloned()
    }

    async fn mark_running(&self, id: &str) {
        if let Some(record) = self.jobs.write().await.get_mut(id) {
            record.status = JobStatus::Running;
            record.started_at_ms = Some(now_ms());
        }
    }

    async fn mark_succeeded(&self, id: &str, report: crate::models::AuditReport) {
        if let Some(record) = self.jobs.write().await.get_mut(id) {
            record.status = JobStatus::Succeeded;
            record.finished_at_ms = Some(now_ms());
            record.result = Some(report);
        }
    }

    async fn mark_failed(&self, id: &str, error: String) {
        if let Some(record) = self.jobs.write().await.get_mut(id) {
            record.status = JobStatus::Failed;
            record.finished_at_ms = Some(now_ms());
            record.error = Some(error);
        }
    }
}
