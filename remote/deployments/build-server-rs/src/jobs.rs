use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

use axum::http::StatusCode;
use tokio::{fs, time::timeout};

use crate::config::Config;
use crate::ecr::login_to_ecr;
use crate::exec::{
    append_log, redacted_build_args, run_logged_command, run_logged_command_inner,
};
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
    let mut jobs = state.jobs.write().await;
    if jobs.len() <= max_jobs {
        return;
    }

    let mut candidates = jobs
        .values()
        .filter(|job| !matches!(job.status, BuildStatus::Queued | BuildStatus::Running))
        .map(|job| (job.created_at_ms, job.id.clone()))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(created_at_ms, _)| *created_at_ms);
    for (_, id) in candidates
        .into_iter()
        .take(jobs.len().saturating_sub(max_jobs))
    {
        jobs.remove(&id);
    }
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

pub(crate) async fn clone_repository(
    config: &Config,
    request: &BuildRequest,
    job_dir: &Path,
    repo_dir: &Path,
    log_path: &Path,
) -> Result<(), String> {
    let mut clone_args = vec![
        "-c".to_string(),
        "protocol.ext.allow=never".to_string(),
        "-c".to_string(),
        "protocol.file.allow=never".to_string(),
        "-c".to_string(),
        "protocol.local.allow=never".to_string(),
        "clone".to_string(),
        "--depth".to_string(),
        "1".to_string(),
        "--no-tags".to_string(),
    ];
    if let Some(git_ref) = clean_optional(request.git_ref.as_deref()) {
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

/// Validate + enqueue a build, shared by the HTTP, webhook, and NATS paths.
/// Applies queue backpressure and best-effort in-process requestId dedupe.
pub(crate) async fn enqueue_build(
    state: &AppState,
    request: BuildRequest,
    source: &str,
) -> Result<BuildJobRecord, (StatusCode, String)> {
    if let Err(error) = validate_build_request(&state.config, &request) {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return Err((StatusCode::BAD_REQUEST, error));
    }

    // In-process dedupe for at-least-once transports. fiducia idempotency
    // leases and the JetStream Nats-Msg-Id are the cross-replica guards; this
    // just collapses a burst of same-process redelivery.
    if let Some(request_id) = clean_optional(request.request_id.as_deref()) {
        let mut seen = state.recent_request_ids.write().await;
        if !seen.insert(request_id.clone()) {
            return Err((
                StatusCode::CONFLICT,
                format!("requestId {request_id} was already accepted"),
            ));
        }
        if seen.len() > 4096 {
            seen.clear();
        }
    }

    // Backpressure: bound the queue so authenticated callers cannot grow
    // memory (and the on-disk job tree) without limit.
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
pub(crate) async fn submit_from_nats(state: &AppState, payload: &[u8]) -> Result<(), NatsSubmitError> {
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
