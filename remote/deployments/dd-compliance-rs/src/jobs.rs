use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::{atomic::AtomicU64, Arc},
};

use tokio::{
    fs,
    io::AsyncWriteExt,
    sync::{RwLock, Semaphore},
};

use crate::{
    audit::run_audit,
    config::Config,
    metrics::Metrics,
    models::{AuditRequest, JobRecord, JobStatus},
    observability::{log_error, log_info},
    util::{next_id, now_ms},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct JobCounts {
    pub queued: usize,
    pub running: usize,
    pub succeeded: usize,
    pub failed: usize,
}

impl JobCounts {
    pub fn total(self) -> usize {
        self.queued + self.running + self.succeeded + self.failed
    }
}

pub struct JobStore {
    jobs: RwLock<BTreeMap<String, JobRecord>>,
    counter: AtomicU64,
    max_jobs: usize,
    work_root: PathBuf,
    concurrency: Arc<Semaphore>,
}

impl JobStore {
    pub async fn load(
        work_root: PathBuf,
        max_jobs: usize,
        max_concurrent_jobs: usize,
    ) -> io::Result<Self> {
        fs::create_dir_all(&work_root).await?;
        let mut jobs = BTreeMap::new();
        let mut interrupted = Vec::new();
        let mut dir = fs::read_dir(&work_root).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if !is_job_record_path(&path) {
                continue;
            }
            let raw = match fs::read_to_string(&path).await {
                Ok(raw) => raw,
                Err(error) => {
                    log_error(
                        "compliance.job_store.read_failed",
                        "failed to read persisted compliance job record",
                        serde_json::json!({ "path": path.display().to_string(), "error": error.to_string() }),
                    );
                    continue;
                }
            };
            let mut record = match serde_json::from_str::<JobRecord>(&raw) {
                Ok(record) => record,
                Err(error) => {
                    log_error(
                        "compliance.job_store.decode_failed",
                        "failed to decode persisted compliance job record",
                        serde_json::json!({ "path": path.display().to_string(), "error": error.to_string() }),
                    );
                    continue;
                }
            };
            if matches!(record.status, JobStatus::Queued | JobStatus::Running) {
                record.status = JobStatus::Failed;
                record.finished_at_ms = Some(now_ms());
                record.error = Some(
                    "job interrupted by service restart before durable worker recovery completed"
                        .to_string(),
                );
                interrupted.push(record.clone());
            }
            jobs.insert(record.id.clone(), record);
        }

        let store = Self {
            jobs: RwLock::new(jobs),
            counter: AtomicU64::new(0),
            max_jobs: max_jobs.max(1),
            work_root,
            concurrency: Arc::new(Semaphore::new(max_concurrent_jobs.max(1))),
        };
        for record in interrupted {
            store.persist_record(&record).await?;
        }
        store.prune_old_records().await;
        Ok(store)
    }

    pub async fn enqueue(
        self: Arc<Self>,
        config: Arc<Config>,
        http: reqwest::Client,
        metrics: Arc<Metrics>,
        request: AuditRequest,
    ) -> Result<JobRecord, String> {
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
        self.persist_record(&record)
            .await
            .map_err(|error| format!("failed to persist queued audit job: {error}"))?;
        {
            let mut jobs = self.jobs.write().await;
            jobs.insert(id.clone(), record.clone());
        }
        self.prune_old_records().await;
        metrics.audit_submitted_total.fetch_add(1);
        log_info(
            "compliance.audit.queued",
            "compliance audit queued",
            serde_json::json!({ "jobId": id, "targetKind": format!("{:?}", request.target.kind) }),
        );

        let store = self.clone();
        tokio::spawn(async move {
            let permit = match store.concurrency.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(error) => {
                    metrics.audit_failed_total.fetch_add(1);
                    metrics.errors_total.fetch_add(1);
                    store
                        .mark_failed(&id, format!("audit worker semaphore closed: {error}"))
                        .await;
                    return;
                }
            };
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
            drop(permit);
        });
        Ok(record)
    }

    pub async fn list(&self) -> Vec<JobRecord> {
        let mut jobs = self.jobs.read().await.values().cloned().collect::<Vec<_>>();
        jobs.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
        jobs
    }

    pub async fn get(&self, id: &str) -> Option<JobRecord> {
        self.jobs.read().await.get(id).cloned()
    }

    pub async fn counts(&self) -> JobCounts {
        let mut counts = JobCounts::default();
        for record in self.jobs.read().await.values() {
            match record.status {
                JobStatus::Queued => counts.queued += 1,
                JobStatus::Running => counts.running += 1,
                JobStatus::Succeeded => counts.succeeded += 1,
                JobStatus::Failed => counts.failed += 1,
            }
        }
        counts
    }

    pub async fn storage_ready(&self) -> io::Result<()> {
        fs::create_dir_all(&self.work_root).await?;
        let ready_path = self.work_root.join(".ready");
        let mut file = fs::File::create(ready_path).await?;
        file.write_all(b"ok\n").await?;
        file.sync_data().await?;
        Ok(())
    }

    pub async fn render_prometheus(&self) -> String {
        let counts = self.counts().await;
        let mut output = String::new();
        output.push_str(
            "# HELP dd_compliance_jobs_current Current compliance audit jobs by status.\n",
        );
        output.push_str("# TYPE dd_compliance_jobs_current gauge\n");
        for (label, value) in [
            ("queued", counts.queued),
            ("running", counts.running),
            ("succeeded", counts.succeeded),
            ("failed", counts.failed),
        ] {
            output.push_str(&format!(
                "dd_compliance_jobs_current{{status=\"{label}\"}} {value}\n"
            ));
        }
        output.push_str("# HELP dd_compliance_jobs_total_current Current compliance audit jobs retained by the service.\n");
        output.push_str("# TYPE dd_compliance_jobs_total_current gauge\n");
        output.push_str(&format!(
            "dd_compliance_jobs_total_current {}\n",
            counts.total()
        ));
        output
    }

    async fn mark_running(&self, id: &str) {
        self.update_record(id, |record| {
            record.status = JobStatus::Running;
            record.started_at_ms = Some(now_ms());
        })
        .await;
    }

    async fn mark_succeeded(&self, id: &str, report: crate::models::AuditReport) {
        self.update_record(id, |record| {
            record.status = JobStatus::Succeeded;
            record.finished_at_ms = Some(now_ms());
            record.result = Some(report);
        })
        .await;
    }

    async fn mark_failed(&self, id: &str, error: String) {
        self.update_record(id, |record| {
            record.status = JobStatus::Failed;
            record.finished_at_ms = Some(now_ms());
            record.error = Some(error);
        })
        .await;
    }

    async fn update_record(&self, id: &str, update: impl FnOnce(&mut JobRecord)) {
        let updated = {
            let mut jobs = self.jobs.write().await;
            let Some(record) = jobs.get_mut(id) else {
                return;
            };
            update(record);
            record.clone()
        };
        if let Err(error) = self.persist_record(&updated).await {
            log_error(
                "compliance.job_store.persist_failed",
                "failed to persist compliance job state",
                serde_json::json!({ "jobId": id, "error": error.to_string() }),
            );
        }
    }

    async fn persist_record(&self, record: &JobRecord) -> io::Result<()> {
        let path = self.job_path(&record.id)?;
        let tmp_path = self
            .work_root
            .join(format!(".{}.{}.tmp", record.id, now_ms()));
        let body = serde_json::to_vec_pretty(record)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        fs::create_dir_all(&self.work_root).await?;
        let mut file = fs::File::create(&tmp_path).await?;
        file.write_all(&body).await?;
        file.write_all(b"\n").await?;
        file.sync_all().await?;
        drop(file);
        fs::rename(tmp_path, path).await
    }

    async fn prune_old_records(&self) {
        let removed_ids = {
            let mut jobs = self.jobs.write().await;
            let mut removed = Vec::new();
            while jobs.len() > self.max_jobs {
                let Some(oldest_id) = jobs
                    .iter()
                    .min_by_key(|(_, record)| record.created_at_ms)
                    .map(|(id, _)| id.clone())
                else {
                    break;
                };
                jobs.remove(&oldest_id);
                removed.push(oldest_id);
            }
            removed
        };
        for id in removed_ids {
            let Ok(path) = self.job_path(&id) else {
                continue;
            };
            if let Err(error) = fs::remove_file(&path).await {
                if error.kind() != io::ErrorKind::NotFound {
                    log_error(
                        "compliance.job_store.prune_failed",
                        "failed to prune old compliance job record",
                        serde_json::json!({ "jobId": id, "path": path.display().to_string(), "error": error.to_string() }),
                    );
                }
            }
        }
    }

    fn job_path(&self, id: &str) -> io::Result<PathBuf> {
        if !is_safe_job_id(id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "job id contains unsafe characters",
            ));
        }
        Ok(self.work_root.join(format!("{id}.json")))
    }
}

fn is_job_record_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension == "json")
}

fn is_safe_job_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::SCHEMA_VERSION,
        models::{AuditTarget, AuditTargetKind},
    };

    fn temp_work_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dd-compliance-rs-{name}-{}", now_ms()))
    }

    fn queued_record(id: &str) -> JobRecord {
        JobRecord {
            id: id.to_string(),
            request_id: format!("request-{id}"),
            status: JobStatus::Running,
            created_at_ms: now_ms(),
            started_at_ms: Some(now_ms()),
            finished_at_ms: None,
            result: None,
            error: None,
        }
    }

    #[tokio::test]
    async fn load_marks_interrupted_jobs_failed() {
        let work_root = temp_work_root("interrupted");
        fs::create_dir_all(&work_root).await.unwrap();
        let record = queued_record("audit-existing");
        fs::write(
            work_root.join("audit-existing.json"),
            serde_json::to_vec_pretty(&record).unwrap(),
        )
        .await
        .unwrap();

        let store = JobStore::load(work_root, 10, 1).await.unwrap();
        let recovered = store.get("audit-existing").await.unwrap();
        assert_eq!(recovered.status, JobStatus::Failed);
        assert!(recovered.finished_at_ms.is_some());
        assert!(recovered.error.unwrap().contains("interrupted"));
    }

    #[tokio::test]
    async fn enqueue_persists_queued_record_before_worker_runs() {
        let work_root = temp_work_root("enqueue");
        let store = Arc::new(JobStore::load(work_root.clone(), 10, 1).await.unwrap());
        let config = Arc::new(Config {
            host: "127.0.0.1".to_string(),
            port: 8118,
            work_root,
            server_auth_secret: Some("secret".to_string()),
            allow_unauthenticated: false,
            allow_external_fetch: false,
            allow_repo_clone: false,
            allow_private_targets: false,
            allowed_repo_prefixes: vec![],
            allowed_file_extensions: vec!["rs".to_string(), "md".to_string()],
            git_bin: "git".to_string(),
            job_timeout: std::time::Duration::from_secs(5),
            max_jobs: 10,
            max_concurrent_jobs: 1,
            max_http_body_bytes: 1024 * 1024,
            max_artifact_bytes: 1024 * 1024,
            max_files: 100,
            max_file_bytes: 1024 * 1024,
            max_findings_per_job: 200,
            max_concurrent_analyses: 4,
        });
        let request = AuditRequest {
            request_id: Some("durable-test".to_string()),
            schema_version: Some(SCHEMA_VERSION.to_string()),
            standard_ids: Some(vec!["soc-2".to_string()]),
            target: AuditTarget {
                kind: AuditTargetKind::Artifact,
                name: None,
                uri: None,
                repo_url: None,
                git_ref: None,
                inline_text: Some("MFA, logging, encryption, incident response".to_string()),
                tags: vec![],
            },
            evidence: vec![],
            options: None,
        };

        let record = store
            .clone()
            .enqueue(
                config,
                reqwest::Client::new(),
                Arc::new(Metrics::default()),
                request,
            )
            .await
            .unwrap();
        assert!(store.job_path(&record.id).unwrap().exists());
        assert!(store.counts().await.total() >= 1);
        store.storage_ready().await.unwrap();
    }
}
