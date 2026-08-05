//! Execute one durable Meshy image-to-3D job through the canonical job-control
//! state machine.
//!
//! The dispatcher supplies a tenant and job id. This process acquires the
//! Fiducia lease, claims the RDS row with the returned fencing token, persists
//! each safe checkpoint, downloads requested artifacts through an explicit host
//! allowlist, and completes with review-blocked archive receipts. JetStream is
//! only a wakeup mechanism; RDS remains the source of truth.

#[path = "../job_control.rs"]
mod job_control;

use std::{
    env,
    error::Error,
    future::Future,
    io,
    time::{Duration, Instant},
};

use dd_meshy_job::{
    ArchiveOutcome, BoundedHttpFetcher, DirectoryArchive, JobIdentity, MeshyGeometryProvider,
    MeshyJobCheckpoint, MeshyJobEngine, MeshyJobError, MeshyJobRequest, MeshyJobResult,
    MeshyJobResultValue, PollOutcome, Preparation, ProviderTaskStatus, SubmissionOutcome, JOB_KIND,
};
use job_control::{ClaimedJobLease, FiduciaClient, JobControlResult, JobStore};

const DEFAULT_POLL_SECS: u64 = 5;
const DEFAULT_MAX_WAIT_SECS: u64 = 30 * 60;
const DEFAULT_LEASE_SECS: u64 = 120;
const DEFAULT_FIDUCIA_WAIT_SECS: u64 = 5;

#[tokio::main]
async fn main() -> JobControlResult<()> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.is_empty() || arguments.iter().any(|argument| argument == "--help") {
        print_usage();
        return Ok(());
    }
    if arguments.first().map(String::as_str) != Some("run-one") {
        return Err(invalid("expected command 'run-one'"));
    }

    let tenant_id = required_flag(&arguments, "--tenant")?;
    let job_id = required_flag(&arguments, "--job-id")?;
    let owner = optional_flag(&arguments, "--owner")
        .or_else(|| env_nonempty("MESHY_WORKER_OWNER"))
        .unwrap_or_else(|| format!("meshy-worker-{}", std::process::id()));

    let store = JobStore::connect_from_env().await?;
    store.ensure_schema().await?;
    let fiducia = FiduciaClient::from_env()?;
    let lease_seconds =
        bounded_env_u64("FABRICATION_JOB_LEASE_SECS", DEFAULT_LEASE_SECS, 15, 86_400)?;
    let wait_seconds = bounded_env_u64("FIDUCIA_WAIT_SECS", DEFAULT_FIDUCIA_WAIT_SECS, 0, 300)?;
    let lease = ClaimedJobLease::acquire(
        store,
        fiducia,
        &tenant_id,
        &job_id,
        &owner,
        lease_seconds,
        wait_seconds,
    )
    .await?;

    run_claimed_job(lease).await
}

async fn run_claimed_job(mut lease: ClaimedJobLease) -> JobControlResult<()> {
    match process_job(&mut lease).await {
        Ok(result) => {
            let value = serde_json::to_value(result)?;
            let completed = lease.complete(&value).await?;
            println!("{}", serde_json::to_string_pretty(&completed)?);
            Ok(())
        }
        Err(WorkerFailure::Job(error)) => {
            // Ambiguous provider creates are never placed back on the automatic
            // retry path. A human or reconciliation agent must locate/adopt the
            // provider task and enqueue it as `existing_provider_task`.
            let retryable = error.retryable && !error.submission_ambiguous;
            let code = error.code.clone();
            let message = error.message.clone();
            let failed = lease.fail(&code, &message, retryable).await?;
            eprintln!("{}", serde_json::to_string_pretty(&failed)?);
            Err(Box::new(error))
        }
        Err(WorkerFailure::Control(error)) => {
            // Best effort only: the canonical RDS lease remains bounded and the
            // reaper will recover it even when Fiducia release also fails.
            let _ = lease.release().await;
            Err(error)
        }
    }
}

async fn process_job(lease: &mut ClaimedJobLease) -> Result<MeshyJobResult, WorkerFailure> {
    let job = lease.job().clone();
    if job.kind != JOB_KIND {
        return Err(MeshyJobError::new(
            "unexpected_job_kind",
            format!("expected job kind {JOB_KIND:?}, got {:?}", job.kind),
            false,
        )
        .into());
    }

    let request: MeshyJobRequest =
        serde_json::from_value(job.request_payload.clone()).map_err(|error| {
            MeshyJobError::new(
                "invalid_meshy_job_request",
                format!("could not decode Meshy request payload: {error}"),
                false,
            )
        })?;
    request.validate()?;
    let identity = JobIdentity {
        tenant_id: job.tenant_id.clone(),
        job_id: job.job_id.clone(),
    };
    identity.validate()?;

    let engine = MeshyJobEngine;
    let provider = MeshyGeometryProvider::from_env()?;
    let fetcher = BoundedHttpFetcher::from_env()?;
    let archive = DirectoryArchive::from_env()?;
    let heartbeat_every = lease.heartbeat_interval();
    let poll_every = Duration::from_secs(bounded_env_u64(
        "MESHY_WORKER_POLL_SECS",
        DEFAULT_POLL_SECS,
        1,
        300,
    )?);
    let max_wait = Duration::from_secs(bounded_env_u64(
        "MESHY_WORKER_MAX_WAIT_SECS",
        DEFAULT_MAX_WAIT_SECS,
        poll_every.as_secs(),
        24 * 60 * 60,
    )?);

    let existing = MeshyJobCheckpoint::from_value(&job.checkpoint)?;
    let mut checkpoint = match existing {
        Some(checkpoint) => checkpoint,
        None => match engine.prepare(&identity, &request, chrono::Utc::now().timestamp_millis())? {
            Preparation::Adopted { checkpoint } => {
                persist_checkpoint(lease, "meshy_provider_submitted", &checkpoint).await?;
                checkpoint
            }
            Preparation::Submit {
                checkpoint: prepared,
                permit,
            } => {
                // This checkpoint must commit before the billable create call.
                // If the process dies after this line, the ephemeral permit is
                // gone and a restarted worker will refuse to submit again.
                persist_checkpoint(lease, "meshy_submission_prepared", &prepared).await?;
                let outcome = with_heartbeats(
                    lease,
                    heartbeat_every,
                    engine.submit_prepared(
                        &provider,
                        &request,
                        prepared,
                        permit,
                        chrono::Utc::now().timestamp_millis(),
                    ),
                )
                .await?;
                match outcome {
                    SubmissionOutcome::Submitted { checkpoint } => {
                        let task_id = checkpoint
                            .provider_task_id()
                            .unwrap_or("<missing-provider-task-id>")
                            .to_string();
                        let checkpoint_value = checkpoint.to_value()?;
                        if let Err(error) = lease
                            .checkpoint("meshy_provider_submitted", &checkpoint_value)
                            .await
                        {
                            return Err(WorkerFailure::Control(Box::new(io::Error::new(
                                io::ErrorKind::Other,
                                format!(
                                    "Meshy task {task_id} was created but its durable checkpoint failed; do not resubmit automatically, reconcile this provider task id: {error}"
                                ),
                            ))));
                        }
                        checkpoint
                    }
                    SubmissionOutcome::Ambiguous { checkpoint, error } => {
                        persist_checkpoint(lease, "meshy_submission_ambiguous", &checkpoint)
                            .await?;
                        return Err(error.into());
                    }
                }
            }
        },
    };

    let provider_complete = checkpoint
        .provider
        .as_ref()
        .is_some_and(|provider| provider.status == ProviderTaskStatus::Succeeded);
    if !provider_complete {
        let deadline = Instant::now() + max_wait;
        loop {
            let outcome = with_heartbeats(
                lease,
                heartbeat_every,
                engine.poll(&provider, &request, checkpoint),
            )
            .await?;
            match outcome {
                PollOutcome::Waiting {
                    checkpoint: updated,
                } => {
                    let progress = updated
                        .provider
                        .as_ref()
                        .map(|provider| provider.progress)
                        .unwrap_or(0);
                    persist_checkpoint(lease, "meshy_provider_running", &updated).await?;
                    checkpoint = updated;
                    if Instant::now() >= deadline {
                        return Err(MeshyJobError::new(
                            "meshy_worker_wait_timeout",
                            format!(
                                "provider task remained incomplete at {progress}% after {} seconds",
                                max_wait.as_secs()
                            ),
                            true,
                        )
                        .into());
                    }
                    sleep_with_heartbeats(lease, poll_every, heartbeat_every).await?;
                }
                PollOutcome::Succeeded {
                    checkpoint: updated,
                } => {
                    persist_checkpoint(lease, "meshy_provider_succeeded", &updated).await?;
                    checkpoint = updated;
                    break;
                }
            }
        }
    }

    loop {
        let outcome = with_heartbeats(
            lease,
            heartbeat_every,
            engine.archive_next(&fetcher, &archive, &identity, &request, checkpoint),
        )
        .await?;
        match outcome {
            ArchiveOutcome::Archived {
                format,
                artifact,
                checkpoint: updated,
            } => {
                let stage = format!("meshy_artifact_archived_{format}");
                persist_checkpoint(lease, &stage, &updated).await?;
                println!(
                    "archived format={} sha256={} bytes={} uri={}",
                    format, artifact.sha256, artifact.byte_count, artifact.archive_uri
                );
                checkpoint = updated;
            }
            ArchiveOutcome::Complete { result } => return Ok(result),
        }
    }
}

async fn persist_checkpoint(
    lease: &mut ClaimedJobLease,
    stage: &str,
    checkpoint: &MeshyJobCheckpoint,
) -> Result<(), WorkerFailure> {
    let value = checkpoint.to_value()?;
    lease
        .checkpoint(stage, &value)
        .await
        .map_err(WorkerFailure::Control)?;
    Ok(())
}

async fn with_heartbeats<T, F>(
    lease: &mut ClaimedJobLease,
    heartbeat_every: Duration,
    future: F,
) -> Result<T, WorkerFailure>
where
    F: Future<Output = MeshyJobResultValue<T>>,
{
    tokio::pin!(future);
    loop {
        tokio::select! {
            result = &mut future => return result.map_err(WorkerFailure::Job),
            _ = tokio::time::sleep(heartbeat_every) => {
                lease.heartbeat().await.map_err(WorkerFailure::Control)?;
            }
        }
    }
}

async fn sleep_with_heartbeats(
    lease: &mut ClaimedJobLease,
    duration: Duration,
    heartbeat_every: Duration,
) -> Result<(), WorkerFailure> {
    let deadline = Instant::now() + duration;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        tokio::time::sleep(remaining.min(heartbeat_every)).await;
        if Instant::now() < deadline {
            lease.heartbeat().await.map_err(WorkerFailure::Control)?;
        }
    }
}

#[derive(Debug)]
enum WorkerFailure {
    Job(MeshyJobError),
    Control(Box<dyn Error + Send + Sync>),
}

impl From<MeshyJobError> for WorkerFailure {
    fn from(error: MeshyJobError) -> Self {
        Self::Job(error)
    }
}

fn bounded_env_u64(name: &str, default: u64, minimum: u64, maximum: u64) -> JobControlResult<u64> {
    let value = match env::var(name) {
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .map_err(|error| invalid(format!("{name} must be an unsigned integer: {error}")))?,
        Err(_) => default,
    };
    if value < minimum || value > maximum {
        return Err(invalid(format!(
            "{name} must be between {minimum} and {maximum}"
        )));
    }
    Ok(value)
}

fn env_nonempty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn required_flag(arguments: &[String], name: &str) -> JobControlResult<String> {
    optional_flag(arguments, name).ok_or_else(|| invalid(format!("missing required {name}")))
}

fn optional_flag(arguments: &[String], name: &str) -> Option<String> {
    arguments
        .windows(2)
        .find(|window| window[0] == name)
        .map(|window| window[1].trim().to_string())
        .filter(|value| !value.is_empty())
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

fn print_usage() {
    println!(
        "meshy-job-worker\n\n\
         Usage:\n  \
           meshy-job-worker run-one --tenant <tenant> --job-id <uuid> [--owner <worker>]\n\n\
         Required environment:\n  \
           FABRICATION_DATABASE_URL (or RDS_DATABASE_URL / DATABASE_URL)\n  \
           FIDUCIA_BASE_URL (FIDUCIA_AUTH_TOKEN when the service requires auth)\n  \
           MESHY_API_KEY\n  \
           MESHY_ARTIFACT_ALLOWED_HOSTS (comma-separated DNS suffixes)\n\n\
         Artifact storage:\n  \
           MESHY_ARTIFACT_STAGING_DIR (default /var/lib/daedalus/meshy-staging)\n  \
           DAEDALUS_ARTIFACT_ARCHIVE_DIR (default /var/lib/daedalus/artifacts)\n  \
           DAEDALUS_ARTIFACT_URI_PREFIX (default daedalus-artifact://archive)\n\n\
         Timing:\n  \
           FABRICATION_JOB_LEASE_SECS (default 120)\n  \
           FIDUCIA_WAIT_SECS (default 5)\n  \
           MESHY_WORKER_POLL_SECS (default 5)\n  \
           MESHY_WORKER_MAX_WAIT_SECS (default 1800)\n"
    );
}
