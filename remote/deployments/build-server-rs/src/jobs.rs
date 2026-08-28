use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

use axum::http::StatusCode;
use tokio::{fs, time::timeout};

use crate::config::Config;
use crate::ecr::login_to_ecr;
use crate::exec::{append_log, redacted_build_args, run_logged_command, run_logged_command_inner};
use crate::state::{AppState, SERVICE_NAME};
use crate::types::{BuildJobRecord, BuildRequest, BuildStatus, NatsSubmitError};
use crate::util::{now_ms, sha256_hex};
use crate::validation::{
    clean_optional, request_job_kind, validate_build_request, validate_image,
    validate_relative_path, validate_rollout_resource,
};
use crate::{db, events, fiducia, lambda_exec, profiles};

pub(crate) fn job_id(counter: u64) -> String {
    format!("build-{}-{counter}", now_ms())
}

pub(crate) async fn update_job<F>(state: &AppState, id: &str, mutate: F)
where
    F: FnOnce(&mut BuildJobRecord),
{
    let updated = {
        let mut jobs = state.jobs.write().await;
        match jobs.get_mut(id) {
            Some(job) => {
                mutate(job);
                Some(job.clone())
            }
            None => None,
        }
    };
    if let Some(job) = updated {
        if let Some(db) = state.db.as_ref() {
            db::persist_job(db, &job).await;
        }
        events::publish_lifecycle(state, &job).await;
    }
}

pub(crate) async fn prune_jobs(state: &AppState) {
    let max_jobs = state.config.max_jobs;
    // Keep a single lock order everywhere that needs both structures:
    // request ids first, then jobs. This prevents deadlocks and keeps the
    // retained-job snapshot atomic with request-id cleanup.
    let mut request_ids = state.recent_request_ids.write().await;
    let mut jobs = state.jobs.write().await;
    if jobs.len() > max_jobs {
        let mut candidates = jobs
            .values()
            .filter(|job| !matches!(job.status, BuildStatus::Queued | BuildStatus::Running))
            .map(|job| (job.created_at_ms, job.id.clone()))
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(created_at_ms, _)| *created_at_ms);
        let remove_count = jobs.len().saturating_sub(max_jobs);
        for (_, id) in candidates.into_iter().take(remove_count) {
            jobs.remove(&id);
        }
    }

    let retained_request_ids = jobs
        .values()
        .filter_map(|job| clean_optional(job.request.request_id.as_deref()))
        .collect::<HashSet<_>>();
    request_ids.retain(|request_id| retained_request_ids.contains(request_id));
}

pub(crate) async fn resolve_repo_path(
    repo_dir: &Path,
    name: &str,
    value: &str,
) -> Result<PathBuf, String> {
    let clean = validate_relative_path(name, value)?;
    let candidate = repo_dir.join(clean);
    // Canonicalize both sides and require the resolved target to stay under the
    // cloned repo. This defeats an in-repo symlink (e.g. `ctx -> /`) that would
    // otherwise redirect the build context, Dockerfile, or deploy manifest to a
    // host path outside the workspace — critical because the pod mounts the
    // host containerd/buildkit sockets. The clone always runs first, so these
    // paths exist by the time we resolve them.
    let repo_root = tokio::fs::canonicalize(repo_dir)
        .await
        .map_err(|error| format!("failed to resolve repository root: {error}"))?;
    let resolved = tokio::fs::canonicalize(&candidate).await.map_err(|error| {
        format!("{name} {value:?} could not be resolved inside the repository: {error}")
    })?;
    if !resolved.starts_with(&repo_root) {
        return Err(format!("{name} must stay inside the repository root"));
    }
    Ok(resolved)
}

fn is_full_commit_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn restricted_git_args() -> Vec<String> {
    vec![
        "-c".to_string(),
        "protocol.ext.allow=never".to_string(),
        "-c".to_string(),
        "protocol.file.allow=never".to_string(),
        "-c".to_string(),
        "protocol.local.allow=never".to_string(),
    ]
}

pub(crate) async fn clone_repository(
    config: &Config,
    request: &BuildRequest,
    job_dir: &Path,
    repo_dir: &Path,
    log_path: &Path,
) -> Result<(), String> {
    let git_ref = clean_optional(request.git_ref.as_deref());

    // `git clone --branch` accepts branch and tag names, not a raw object ID.
    // Webhook and gha-indie-worker requests intentionally carry an immutable
    // commit OID, so initialize a repository, fetch exactly that object, and
    // detach HEAD at FETCH_HEAD. This prevents a moving branch from changing
    // the code after a webhook or workflow plan has been accepted.
    if let Some(commit_oid) = git_ref.as_deref().filter(|value| is_full_commit_oid(value)) {
        let mut init_args = restricted_git_args();
        init_args.extend([
            "init".to_string(),
            "--".to_string(),
            repo_dir.to_string_lossy().to_string(),
        ]);
        run_logged_command(config, log_path, job_dir, &config.git_bin, init_args).await?;

        run_logged_command(
            config,
            log_path,
            job_dir,
            &config.git_bin,
            vec![
                "-C".to_string(),
                repo_dir.to_string_lossy().to_string(),
                "remote".to_string(),
                "add".to_string(),
                "origin".to_string(),
                request.repo_url.clone(),
            ],
        )
        .await?;

        let mut fetch_args = restricted_git_args();
        fetch_args.extend([
            "-C".to_string(),
            repo_dir.to_string_lossy().to_string(),
            "fetch".to_string(),
            "--depth".to_string(),
            "1".to_string(),
            "--no-tags".to_string(),
            "--no-recurse-submodules".to_string(),
            "origin".to_string(),
            commit_oid.to_string(),
        ]);
        run_logged_command(config, log_path, job_dir, &config.git_bin, fetch_args).await?;

        return run_logged_command(
            config,
            log_path,
            job_dir,
            &config.git_bin,
            vec![
                "-C".to_string(),
                repo_dir.to_string_lossy().to_string(),
                "checkout".to_string(),
                "--detach".to_string(),
                "--force".to_string(),
                "FETCH_HEAD".to_string(),
            ],
        )
        .await;
    }

    let mut clone_args = restricted_git_args();
    clone_args.extend([
        "clone".to_string(),
        "--depth".to_string(),
        "1".to_string(),
        "--no-tags".to_string(),
    ]);
    if let Some(git_ref) = git_ref {
        clone_args.push("--branch".to_string());
        clone_args.push(git_ref);
    }
    clone_args.push("--".to_string());
    clone_args.push(request.repo_url.clone());
    clone_args.push(repo_dir.to_string_lossy().to_string());
    run_logged_command(config, log_path, job_dir, &config.git_bin, clone_args).await
}

pub(crate) async fn execute_profile(state: &AppState, job: &BuildJobRecord) -> Result<(), String> {
    let config = state.config.as_ref();
    let request = &job.request;
    let profile_name = clean_optional(request.profile.as_deref())
        .ok_or_else(|| "validated profile request lost its profile".to_string())?;
    let profile = profiles::find(&profile_name)
        .ok_or_else(|| format!("profile {profile_name:?} is not installed"))?;
    let job_dir = config.work_root.join(&job.id);
    let repo_dir = job_dir.join("repo");
    let log_path = PathBuf::from(&job.log_path);

    fs::create_dir_all(&job_dir)
        .await
        .map_err(|error| format!("failed to create job dir: {error}"))?;
    append_log(
        &log_path,
        &format!(
            "{SERVICE_NAME} starting profile job={} repo={} profile={}\n",
            job.id, request.repo_url, profile.name
        ),
        config.max_log_bytes,
    )
    .await;
    clone_repository(config, request, &job_dir, &repo_dir, &log_path).await?;

    let context_path = resolve_repo_path(
        &repo_dir,
        "contextDir",
        request.context_dir.as_deref().unwrap_or("."),
    )
    .await?;
    for step in profile.steps {
        let step_cwd = validate_relative_path("profile step subdirectory", step.subdirectory)?;
        let container_cwd = if step_cwd == Path::new(".") {
            "/workspace".to_string()
        } else {
            format!("/workspace/{}", step_cwd.to_string_lossy())
        };
        append_log(
            &log_path,
            &format!(
                "\nprofile={} step={} runner={}\n",
                profile.name, step.name, step.image
            ),
            config.max_log_bytes,
        )
        .await;
        let mut runner_args = vec![
            "-n".to_string(),
            config.containerd_namespace.clone(),
            "run".to_string(),
            "--rm".to_string(),
            "--pull=missing".to_string(),
            format!("--cpus={}", config.profile_cpus),
            format!("--memory={}", config.profile_memory),
            format!("--pids-limit={}", config.profile_pids_limit),
            "--security-opt=no-new-privileges".to_string(),
            "--cap-drop=ALL".to_string(),
        ];
        runner_args.extend([
            "--env=CI=true".to_string(),
            "--mount".to_string(),
            format!(
                "type=bind,src={},dst=/workspace",
                context_path.to_string_lossy()
            ),
            "--workdir".to_string(),
            container_cwd,
            step.image.to_string(),
            "/bin/bash".to_string(),
            "-lc".to_string(),
            step.script.to_string(),
        ]);
        run_logged_command(
            config,
            &log_path,
            &context_path,
            &config.nerdctl_bin,
            runner_args,
        )
        .await?;
    }

    if !profile.artifact_paths.is_empty() {
        let archive_path = job_dir.join("artifacts.tar.gz");
        let mut args = vec![
            "-czf".to_string(),
            archive_path.to_string_lossy().to_string(),
            "--".to_string(),
        ];
        args.extend(profile.artifact_paths.iter().map(|path| path.to_string()));
        run_logged_command(config, &log_path, &context_path, &config.tar_bin, args).await?;
        append_log(
            &log_path,
            &format!("artifacts: /builds/{}/artifacts\n", job.id),
            config.max_log_bytes,
        )
        .await;
    }

    append_log(
        &log_path,
        &format!("{SERVICE_NAME} completed profile job={}\n", job.id),
        config.max_log_bytes,
    )
    .await;
    Ok(())
}

pub(crate) async fn execute_build(state: &AppState, job: &BuildJobRecord) -> Result<(), String> {
    let config = state.config.as_ref();
    let request = &job.request;
    let job_dir = config.work_root.join(&job.id);
    let repo_dir = job_dir.join("repo");
    let log_path = PathBuf::from(&job.log_path);

    fs::create_dir_all(&job_dir)
        .await
        .map_err(|error| format!("failed to create job dir: {error}"))?;
    append_log(
        &log_path,
        &format!(
            "{SERVICE_NAME} starting job={} repo={} image={}\n",
            job.id, request.repo_url, request.image
        ),
        config.max_log_bytes,
    )
    .await;

    // Locked-down clone: no non-network transports, no tags, and an explicit
    // `--` so nothing user-supplied can ever be parsed as a git option.
    clone_repository(config, request, &job_dir, &repo_dir, &log_path).await?;

    let context_path = resolve_repo_path(
        &repo_dir,
        "contextDir",
        request.context_dir.as_deref().unwrap_or("."),
    )
    .await?;
    let dockerfile_path = resolve_repo_path(
        &repo_dir,
        "dockerfile",
        request.dockerfile.as_deref().unwrap_or("Dockerfile"),
    )
    .await?;

    let mut build_args = vec![
        "-n".to_string(),
        config.containerd_namespace.clone(),
        "build".to_string(),
        "-f".to_string(),
        dockerfile_path.to_string_lossy().to_string(),
        "-t".to_string(),
        request.image.clone(),
    ];
    if let Some(args) = &request.build_args {
        for (key, value) in args {
            build_args.push("--build-arg".to_string());
            build_args.push(format!("{key}={value}"));
        }
    }
    build_args.push(context_path.to_string_lossy().to_string());
    let display_build_args = redacted_build_args(&build_args);
    run_logged_command_inner(
        config,
        &log_path,
        &repo_dir,
        &config.nerdctl_bin,
        build_args,
        Some(display_build_args),
        None,
    )
    .await?;

    if request.push.unwrap_or(false) {
        let ecr = validate_image(config, &request.image, true)?;
        if config.ecr_login_enabled {
            let ecr = ecr.ok_or_else(|| "push requires an ECR image".to_string())?;
            match login_to_ecr(state, &log_path, &repo_dir, &ecr).await {
                Ok(()) => {
                    state.counters.ecr_logins.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    state
                        .counters
                        .ecr_login_failures
                        .fetch_add(1, Ordering::Relaxed);
                    return Err(error);
                }
            }
        }
        run_logged_command(
            config,
            &log_path,
            &repo_dir,
            &config.nerdctl_bin,
            vec![
                "-n".to_string(),
                config.containerd_namespace.clone(),
                "push".to_string(),
                request.image.clone(),
            ],
        )
        .await?;
    }

    if let Some(deploy) = &request.deploy {
        if deploy.kind != "none" {
            let namespace = deploy.namespace.as_deref().unwrap_or("default");
            let deploy_path = resolve_repo_path(&repo_dir, "deploy.path", &deploy.path).await?;
            let mut apply_args = vec!["-n".to_string(), namespace.to_string(), "apply".to_string()];
            match deploy.kind.as_str() {
                "kustomize" => {
                    apply_args.push("-k".to_string());
                    apply_args.push(deploy_path.to_string_lossy().to_string());
                }
                "manifest" => {
                    apply_args.push("-f".to_string());
                    apply_args.push(deploy_path.to_string_lossy().to_string());
                }
                _ => unreachable!("deploy kind is validated before queueing"),
            }
            run_logged_command(
                config,
                &log_path,
                &repo_dir,
                &config.kubectl_bin,
                apply_args,
            )
            .await?;

            if let Some(rollout) = deploy.rollout.as_deref() {
                let resource = validate_rollout_resource(rollout)?;
                let timeout_seconds = deploy.rollout_timeout_seconds.unwrap_or(300);
                run_logged_command(
                    config,
                    &log_path,
                    &repo_dir,
                    &config.kubectl_bin,
                    vec![
                        "-n".to_string(),
                        namespace.to_string(),
                        "rollout".to_string(),
                        "status".to_string(),
                        resource,
                        format!("--timeout={timeout_seconds}s"),
                    ],
                )
                .await?;
            }
        }
    }

    append_log(
        &log_path,
        &format!("{SERVICE_NAME} completed job={}\n", job.id),
        config.max_log_bytes,
    )
    .await;
    Ok(())
}

pub(crate) async fn run_job(state: AppState, id: String) {
    let permit = match state.semaphore.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(error) => {
            update_job(&state, &id, |job| {
                job.status = BuildStatus::Failed;
                job.finished_at_ms = Some(now_ms());
                job.error = Some(format!("build queue is closed: {error}"));
            })
            .await;
            return;
        }
    };

    // Distributed mutual exclusion (fiducia.cloud): one lock per image ref, so
    // concurrent builds of the same image serialize across replicas. The local
    // semaphore above only bounds this process.
    let lock_key = {
        let jobs = state.jobs.read().await;
        jobs.get(&id).map(|job| {
            if request_job_kind(&job.request) == "run-profile" {
                format!(
                    "build-server/profile/{}/{}",
                    job.request.profile.as_deref().unwrap_or("unknown"),
                    sha256_hex(&job.request.repo_url)
                )
            } else {
                format!("build-server/image/{}", job.request.image)
            }
        })
    };
    let mut grant: Option<fiducia::LockGrant> = None;
    if let Some(lock_key) = lock_key.as_deref() {
        match fiducia::acquire_lock(&state.http, &state.config, lock_key, &state.holder).await {
            fiducia::LockOutcome::Disabled => {}
            fiducia::LockOutcome::Acquired(acquired) => {
                state
                    .counters
                    .locks_acquired
                    .fetch_add(1, Ordering::Relaxed);
                grant = Some(acquired);
            }
            fiducia::LockOutcome::Busy { key } => {
                state.counters.lock_failures.fetch_add(1, Ordering::Relaxed);
                state.counters.failed.fetch_add(1, Ordering::Relaxed);
                update_job(&state, &id, |job| {
                    job.status = BuildStatus::Failed;
                    job.finished_at_ms = Some(now_ms());
                    job.error = Some(format!(
                        "another build holds the fiducia lock for {key}; retry later"
                    ));
                })
                .await;
                drop(permit);
                return;
            }
            fiducia::LockOutcome::Unavailable { error } => {
                state.counters.lock_failures.fetch_add(1, Ordering::Relaxed);
                if state.config.coordination_required {
                    state.counters.failed.fetch_add(1, Ordering::Relaxed);
                    update_job(&state, &id, |job| {
                        job.status = BuildStatus::Failed;
                        job.finished_at_ms = Some(now_ms());
                        job.error = Some(format!(
                            "fiducia coordination is required but unavailable: {error}"
                        ));
                    })
                    .await;
                    drop(permit);
                    return;
                }
                tracing::warn!(
                    "fiducia coordination unavailable, continuing with local semaphore only: {error}"
                );
            }
        }
    }

    state.counters.running.fetch_add(1, Ordering::Relaxed);
    let grant_key = grant.as_ref().map(|grant| grant.key.clone());
    let grant_token = grant.as_ref().map(|grant| grant.fencing_token);
    update_job(&state, &id, |job| {
        job.status = BuildStatus::Running;
        job.started_at_ms = Some(now_ms());
        job.lock_key = grant_key.clone();
        job.fencing_token = grant_token;
    })
    .await;

    let job = {
        let jobs = state.jobs.read().await;
        jobs.get(&id).cloned()
    };

    let result = match job {
        Some(job) => {
            // Hard wall-clock deadline for the whole job, on top of the
            // per-command timeout inside the executors.
            let deadline = state.config.job_deadline;
            let execution = async {
                if request_job_kind(&job.request) == "run-profile" {
                    execute_profile(&state, &job).await
                } else if job.executor == "lambda" {
                    lambda_exec::execute(&state, &job, Path::new(&job.log_path)).await
                } else {
                    execute_build(&state, &job).await
                }
            };
            match timeout(deadline, execution).await {
                Ok(result) => result,
                Err(_) => Err(format!("job exceeded overall deadline of {deadline:?}")),
            }
        }
        None => Err("job disappeared before execution".to_string()),
    };

    if let Some(grant) = grant.as_ref() {
        fiducia::release_lock(&state.http, &state.config, grant).await;
    }

    // Workdir GC: the cloned repo is scratch space; keep only the build log
    // unless the operator opts into keeping workdirs for debugging.
    if !state.config.keep_workdirs {
        let repo_dir = state.config.work_root.join(&id).join("repo");
        if let Err(error) = fs::remove_dir_all(&repo_dir).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("failed to remove workdir {}: {error}", repo_dir.display());
            }
        }
    }

    state.counters.running.fetch_sub(1, Ordering::Relaxed);
    drop(permit);

    match result {
        Ok(()) => {
            state.counters.succeeded.fetch_add(1, Ordering::Relaxed);
            update_job(&state, &id, |job| {
                job.status = BuildStatus::Succeeded;
                job.finished_at_ms = Some(now_ms());
                job.error = None;
            })
            .await;
        }
        Err(error) => {
            state.counters.failed.fetch_add(1, Ordering::Relaxed);
            state
                .counters
                .command_failures
                .fetch_add(1, Ordering::Relaxed);
            update_job(&state, &id, |job| {
                job.status = BuildStatus::Failed;
                job.finished_at_ms = Some(now_ms());
                job.error = Some(error);
            })
            .await;
        }
    }
}

fn requests_match(left: &BuildRequest, right: &BuildRequest) -> bool {
    // BuildRequest contains only serializable, deterministic data. Comparing
    // its JSON value makes request-id reuse fail closed when a caller changes
    // any execution input while preserving the same idempotency identity.
    match (serde_json::to_value(left), serde_json::to_value(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Validate + enqueue a build, shared by the HTTP, webhook, and NATS paths.
/// Applies queue backpressure and in-process requestId idempotency.
pub(crate) async fn enqueue_build(
    state: &AppState,
    request: BuildRequest,
    source: &str,
) -> Result<BuildJobRecord, (StatusCode, String)> {
    if let Err(error) = validate_build_request(&state.config, &request) {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return Err((StatusCode::BAD_REQUEST, error));
    }

    let request_id = clean_optional(request.request_id.as_deref());
    // Hold the request-id writer across duplicate lookup, queue admission, and
    // job insertion. This makes two concurrent deliveries with the same id
    // observe one accepted job rather than racing into two build tasks.
    let mut request_ids = if request_id.is_some() {
        Some(state.recent_request_ids.write().await)
    } else {
        None
    };

    if let (Some(request_id), Some(seen)) = (request_id.as_deref(), request_ids.as_mut()) {
        if seen.contains(request_id) {
            let existing = {
                let jobs = state.jobs.read().await;
                jobs.values()
                    .find(|job| {
                        clean_optional(job.request.request_id.as_deref()).as_deref()
                            == Some(request_id)
                    })
                    .cloned()
            };
            if let Some(existing) = existing {
                if requests_match(&existing.request, &request) {
                    return Ok(existing);
                }
                state.counters.rejected.fetch_add(1, Ordering::Relaxed);
                return Err((
                    StatusCode::CONFLICT,
                    format!("requestId {request_id} is already bound to a different build request"),
                ));
            }

            // A claim without a retained job can exist only after external
            // test manipulation or legacy in-process state. Remove it so a
            // safe retry can be admitted instead of being poisoned forever.
            seen.remove(request_id);
        }
    }

    // Backpressure: bound the queue so authenticated callers cannot grow
    // memory (and the on-disk job tree) without limit. A rejected request does
    // not consume its requestId, so the same delivery can retry later.
    {
        let jobs = state.jobs.read().await;
        let queued = jobs
            .values()
            .filter(|job| matches!(job.status, BuildStatus::Queued))
            .count();
        if queued >= state.config.max_queued {
            state.counters.rejected.fetch_add(1, Ordering::Relaxed);
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "build queue is full ({queued} queued; limit {})",
                    state.config.max_queued
                ),
            ));
        }
    }

    let executor = request
        .executor
        .clone()
        .unwrap_or_else(|| "local".to_string());
    let counter = state.counters.submitted.fetch_add(1, Ordering::Relaxed) + 1;
    let id = job_id(counter);
    let job_dir = state.config.work_root.join(&id);
    let log_path = job_dir.join("build.log");
    let record = BuildJobRecord {
        id: id.clone(),
        status: BuildStatus::Queued,
        request,
        source: source.to_string(),
        executor,
        created_at_ms: now_ms(),
        started_at_ms: None,
        finished_at_ms: None,
        log_path: log_path.to_string_lossy().to_string(),
        error: None,
        lock_key: None,
        fencing_token: None,
    };

    {
        let mut jobs = state.jobs.write().await;
        jobs.insert(id.clone(), record.clone());
    }
    if let (Some(request_id), Some(seen)) = (request_id, request_ids.as_mut()) {
        seen.insert(request_id);
    }
    drop(request_ids);

    if let Some(db) = state.db.as_ref() {
        db::persist_job(db, &record).await;
    }
    prune_jobs(state).await;

    let task_state = state.clone();
    let task_id = id.clone();
    tokio::spawn(async move {
        run_job(task_state, task_id).await;
    });

    Ok(record)
}

/// NATS intake: parse a build-server.v1 document and enqueue it.
pub(crate) async fn submit_from_nats(
    state: &AppState,
    payload: &[u8],
) -> Result<(), NatsSubmitError> {
    let request: BuildRequest = serde_json::from_slice(payload).map_err(|error| {
        NatsSubmitError::Invalid(format!("invalid build request JSON: {error}"))
    })?;
    match enqueue_build(state, request, "nats").await {
        Ok(_) => Ok(()),
        Err((StatusCode::CONFLICT, message)) => {
            tracing::info!("nats build request deduped: {message}");
            Ok(())
        }
        Err((StatusCode::SERVICE_UNAVAILABLE, message)) => Err(NatsSubmitError::Transient(message)),
        Err((_, message)) => Err(NatsSubmitError::Invalid(message)),
    }
}

#[cfg(test)]
mod idempotency_tests {
    use super::*;
    use std::{
        collections::{HashMap, HashSet},
        sync::{atomic::Ordering, Arc},
        time::Duration,
    };

    use tokio::sync::{RwLock, Semaphore};

    use crate::{
        config::Config,
        state::Counters,
        types::{BuildRequest, BuildStatus},
    };

    fn test_config(max_jobs: usize, max_queued: usize) -> Config {
        Config {
            work_root: std::env::temp_dir().join(format!(
                "dd-build-server-idempotency-{}",
                uuid::Uuid::new_v4()
            )),
            git_bin: "git".to_string(),
            git_http_auth_header: None,
            nerdctl_bin: "nerdctl".to_string(),
            kubectl_bin: "kubectl".to_string(),
            tar_bin: "tar".to_string(),
            containerd_namespace: "dd-build-test".to_string(),
            allowed_repo_prefixes: vec!["https://github.com/ORESoftware/".to_string()],
            allowed_image_prefixes: vec![
                "710156900967.dkr.ecr.us-east-1.amazonaws.com/".to_string()
            ],
            allowed_namespaces: HashSet::from(["default".to_string()]),
            allowed_profiles: HashSet::from(["playwright".to_string()]),
            allowed_profile_repo_prefixes: vec!["https://github.com/ORESoftware/".to_string()],
            profile_cpus: "2".to_string(),
            profile_memory: "2g".to_string(),
            profile_pids_limit: "512".to_string(),
            deploy_enabled: true,
            push_enabled: false,
            ecr_login_enabled: false,
            aws_region: "us-east-1".to_string(),
            job_timeout: Duration::from_secs(60),
            job_deadline: Duration::from_secs(120),
            max_log_bytes: 1_000_000,
            max_jobs,
            max_queued,
            keep_workdirs: false,
            server_auth_secret: Some("test-auth".to_string()),
            database_url: None,
            fiducia_url: "http://127.0.0.1:1/unused".to_string(),
            fiducia_api_key: None,
            coordination_enabled: false,
            coordination_required: false,
            lock_ttl: Duration::from_secs(60),
            lock_wait_budget: Duration::from_secs(1),
            lock_retry_interval: Duration::from_millis(10),
            idempotency_lease: Duration::from_secs(60),
            idempotency_retention: Duration::from_secs(60),
            nats_url: "nats://127.0.0.1:4222".to_string(),
            nats_enabled: false,
            nats_intake_enabled: false,
            nats_event_subject: "dd.remote.build_server.events".to_string(),
            nats_result_subject: "dd.remote.build_server.results".to_string(),
            nats_image_subject: "dd.remote.build_server.images".to_string(),
            nats_request_subject: "dd.remote.build_server.requests".to_string(),
            nats_critical_subject: "dd.remote.events.critical".to_string(),
            github_webhook_secret: None,
            registry_webhook_secret: None,
            webhook_rules: Vec::new(),
            gh_sync_enabled: false,
            gh_sync_token: None,
            gh_sync_rules: Vec::new(),
            gh_sync_interval: Duration::ZERO,
            lambda_executor_enabled: false,
            lambda_url: "http://127.0.0.1:1/unused".to_string(),
            lambda_function_id: None,
            lambda_auth_secret: None,
        }
    }

    fn test_state(max_jobs: usize, max_queued: usize) -> AppState {
        AppState {
            config: Arc::new(test_config(max_jobs, max_queued)),
            http: reqwest::Client::new(),
            jobs: Arc::new(RwLock::new(HashMap::new())),
            // Keep accepted jobs queued so these tests never launch git or a
            // container runtime. Tokio cancels the waiting tasks with the test.
            semaphore: Arc::new(Semaphore::new(0)),
            counters: Arc::new(Counters::default()),
            db: None,
            nats: None,
            holder: "dd-build-server/idempotency-test".to_string(),
            recent_request_ids: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    fn profile_request(request_id: &str, revision: &str) -> BuildRequest {
        BuildRequest {
            schema_version: Some("build-server.v1".to_string()),
            job_kind: Some("run-profile".to_string()),
            repo_url: "https://github.com/ORESoftware/k8s-cluster.git".to_string(),
            git_ref: Some(revision.to_string()),
            image: String::new(),
            profile: Some("playwright".to_string()),
            context_dir: None,
            dockerfile: None,
            build_args: None,
            push: Some(false),
            deploy: None,
            executor: Some("local".to_string()),
            request_id: Some(request_id.to_string()),
        }
    }

    #[tokio::test]
    async fn identical_request_id_reattaches_to_one_job() {
        let state = test_state(16, 16);
        let request = profile_request(
            "gha-clone:plan-1:browser",
            "0123456789abcdef0123456789abcdef01234567",
        );

        let first = enqueue_build(&state, request.clone(), "http")
            .await
            .expect("first request accepted");
        let duplicate = enqueue_build(&state, request, "http")
            .await
            .expect("identical request reattaches");

        assert_eq!(duplicate.id, first.id);
        assert_eq!(state.jobs.read().await.len(), 1);
        assert_eq!(
            state.counters.submitted.load(Ordering::Relaxed),
            1,
            "duplicate must not allocate another job or increment submissions"
        );
        assert_eq!(state.recent_request_ids.read().await.len(), 1);
    }

    #[tokio::test]
    async fn changed_payload_cannot_reuse_an_existing_request_id() {
        let state = test_state(16, 16);
        let request_id = "gha-clone:plan-2:rust";
        enqueue_build(
            &state,
            profile_request(request_id, "0123456789abcdef0123456789abcdef01234567"),
            "http",
        )
        .await
        .expect("first request accepted");

        let (status, message) = enqueue_build(
            &state,
            profile_request(request_id, "89abcdef0123456789abcdef0123456789abcdef"),
            "http",
        )
        .await
        .expect_err("changed request must fail closed");

        assert_eq!(status, StatusCode::CONFLICT);
        assert!(message.contains("different build request"));
        assert_eq!(state.jobs.read().await.len(), 1);
        assert_eq!(state.counters.submitted.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn queue_full_rejection_does_not_poison_request_id_retry() {
        let state = test_state(16, 1);
        let blocker = enqueue_build(
            &state,
            profile_request("blocker", "0123456789abcdef0123456789abcdef01234567"),
            "http",
        )
        .await
        .expect("blocker accepted");

        let retry_id = "gha-clone:plan-3:node";
        let (status, _) = enqueue_build(
            &state,
            profile_request(retry_id, "89abcdef0123456789abcdef0123456789abcdef"),
            "http",
        )
        .await
        .expect_err("full queue rejects");
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            !state.recent_request_ids.read().await.contains(retry_id),
            "a transient queue failure must not consume the request id"
        );

        state.jobs.write().await.remove(&blocker.id);
        state.recent_request_ids.write().await.remove("blocker");

        let retry = enqueue_build(
            &state,
            profile_request(retry_id, "89abcdef0123456789abcdef0123456789abcdef"),
            "http",
        )
        .await
        .expect("same request id succeeds after capacity returns");
        assert_eq!(
            retry.request.request_id.as_deref(),
            Some(retry_id),
            "retry retains its deterministic identity"
        );
        assert_eq!(state.counters.submitted.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn pruning_terminal_job_releases_its_request_id_claim() {
        let state = test_state(1, 8);
        let first_request = profile_request(
            "gha-clone:plan-4:first",
            "0123456789abcdef0123456789abcdef01234567",
        );
        let first = enqueue_build(&state, first_request.clone(), "http")
            .await
            .expect("first accepted");
        {
            let mut jobs = state.jobs.write().await;
            jobs.get_mut(&first.id).expect("first retained").status = BuildStatus::Succeeded;
        }

        enqueue_build(
            &state,
            profile_request(
                "gha-clone:plan-4:second",
                "89abcdef0123456789abcdef0123456789abcdef",
            ),
            "http",
        )
        .await
        .expect("second accepted and pruning runs");

        assert!(!state.jobs.read().await.contains_key(&first.id));
        assert!(!state
            .recent_request_ids
            .read()
            .await
            .contains("gha-clone:plan-4:first"));

        let replacement = enqueue_build(&state, first_request, "http")
            .await
            .expect("pruned identity can be reused for a new retained job");
        assert_ne!(replacement.id, first.id);
    }
}

#[cfg(test)]
mod commit_oid_tests {
    use super::is_full_commit_oid;

    #[test]
    fn immutable_commit_detection_accepts_only_full_hex_object_ids() {
        assert!(is_full_commit_oid(
            "0123456789abcdef0123456789abcdef01234567"
        ));
        assert!(is_full_commit_oid(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!is_full_commit_oid("0123456"));
        assert!(!is_full_commit_oid(
            "0123456789abcdef0123456789abcdef0123456"
        ));
        assert!(!is_full_commit_oid(
            "0123456789abcdef0123456789abcdef0123456z"
        ));
    }
}
