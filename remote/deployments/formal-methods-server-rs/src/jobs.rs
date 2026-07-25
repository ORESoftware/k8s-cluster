use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use tokio::{fs, process::Command, time::timeout};

use crate::{
    annotations::parse_annotations,
    github::{post_pr_comment, render_pr_comment_body},
    scan::{analyze_tree, append_log},
    state::{now_ms, AppState, Config},
    types::{Finding, JobRecord, JobStatus, PullRequestRef},
    validation::{clean_optional, ensure_allowed_prefix, validate_relative_path, validate_repo_url},
    verify::{heuristic_checks, verify_block, VerifyContext},
    SERVICE_NAME,
};

// ---------------------------------------------------------------------------
// job orchestration
// ---------------------------------------------------------------------------

pub(crate) fn job_id(counter: u64) -> String {
    format!("formal-{}-{counter}", now_ms())
}

async fn update_job<F>(state: &AppState, id: &str, mutate: F)
where
    F: FnOnce(&mut JobRecord),
{
    let mut jobs = state.jobs.write().await;
    if let Some(job) = jobs.get_mut(id) {
        mutate(job);
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
        .filter(|job| !matches!(job.status, JobStatus::Queued | JobStatus::Running))
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

struct GitOutcome {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

async fn run_git(
    config: &Config,
    log_path: &Path,
    cwd: &Path,
    args: &[&str],
    timeout_dur: Duration,
    record_in_log: bool,
) -> Result<GitOutcome, String> {
    if record_in_log {
        append_log(
            log_path,
            &format!(
                "$ {} -C {} {}\n",
                config.git_bin,
                cwd.display(),
                args.join(" ")
            ),
            config.max_log_bytes,
        )
        .await;
    }
    let output = match timeout(
        timeout_dur,
        Command::new(&config.git_bin)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("HOME", cwd)
            .env(
                "PATH",
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            )
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "/bin/false")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    {
        Ok(Ok(out)) => out,
        Ok(Err(error)) => return Err(format!("git failed to spawn: {error}")),
        Err(_) => return Err(format!("git timed out: {args:?}")),
    };
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if record_in_log {
        if !stdout.is_empty() {
            append_log(log_path, &stdout, config.max_log_bytes).await;
        }
        if !stderr.is_empty() {
            append_log(log_path, &stderr, config.max_log_bytes).await;
        }
    }
    Ok(GitOutcome {
        status: output.status,
        stdout,
        stderr,
    })
}

async fn clone_repo(
    config: &Config,
    log_path: &Path,
    job_dir: &Path,
    repo_url: &str,
    git_ref: Option<&str>,
) -> Result<PathBuf, String> {
    let repo_dir = job_dir.join("repo");
    let mut clone_args: Vec<String> = vec!["clone".into(), "--depth".into(), "1".into()];
    if let Some(git_ref) = git_ref {
        clone_args.push("--branch".into());
        clone_args.push(git_ref.to_string());
    }
    clone_args.push(repo_url.to_string());
    clone_args.push(repo_dir.to_string_lossy().to_string());
    let arg_refs: Vec<&str> = clone_args.iter().map(String::as_str).collect();
    let outcome = run_git(
        config,
        log_path,
        job_dir,
        &arg_refs,
        config.job_timeout,
        true,
    )
    .await?;
    if !outcome.status.success() {
        return Err(format!("git clone exited with status {}", outcome.status));
    }
    Ok(repo_dir)
}

fn is_sha_like(value: &str) -> bool {
    let len = value.len();
    (4..=64).contains(&len) && value.chars().all(|c| c.is_ascii_hexdigit())
}

async fn clone_for_pr(
    config: &Config,
    log_path: &Path,
    job_dir: &Path,
    pr: &PullRequestRef,
) -> Result<(PathBuf, Option<Vec<String>>), String> {
    let repo_dir = job_dir.join("repo");
    fs::create_dir_all(&repo_dir)
        .await
        .map_err(|error| format!("failed to create pr repo dir: {error}"))?;

    if !is_sha_like(&pr.head_sha) {
        return Err("pull_request.head_sha must be a hex SHA".to_string());
    }
    if !is_sha_like(&pr.base_sha) {
        return Err("pull_request.base_sha must be a hex SHA".to_string());
    }
    validate_repo_url(&pr.head_clone_url)?;
    ensure_allowed_prefix(
        "pull_request.head_clone_url",
        &pr.head_clone_url,
        &config.allowed_repo_prefixes,
        "FORMAL_METHODS_ALLOWED_REPO_PREFIXES",
    )?;

    append_log(
        log_path,
        &format!(
            "{SERVICE_NAME} PR {}/{}#{}: head={} base={}\n",
            pr.owner, pr.repo, pr.number, pr.head_sha, pr.base_sha
        ),
        config.max_log_bytes,
    )
    .await;

    let init = run_git(
        config,
        log_path,
        &repo_dir,
        &["init", "-q"],
        Duration::from_secs(30),
        true,
    )
    .await?;
    if !init.status.success() {
        return Err(format!("git init failed: {}", init.stderr));
    }
    let remote = run_git(
        config,
        log_path,
        &repo_dir,
        &["remote", "add", "origin", &pr.head_clone_url],
        Duration::from_secs(15),
        true,
    )
    .await?;
    if !remote.status.success() {
        return Err(format!("git remote add failed: {}", remote.stderr));
    }
    let head_fetch = run_git(
        config,
        log_path,
        &repo_dir,
        &["fetch", "--depth", "1", "origin", &pr.head_sha],
        config.job_timeout,
        true,
    )
    .await?;
    if !head_fetch.status.success() {
        return Err(format!(
            "git fetch <head_sha> failed: {}",
            head_fetch.stderr
        ));
    }
    let checkout = run_git(
        config,
        log_path,
        &repo_dir,
        &["checkout", "-q", &pr.head_sha],
        Duration::from_secs(60),
        true,
    )
    .await?;
    if !checkout.status.success() {
        return Err(format!("git checkout failed: {}", checkout.stderr));
    }

    let depth = config.pr_base_fetch_depth.to_string();
    let base_fetch = run_git(
        config,
        log_path,
        &repo_dir,
        &["fetch", "--depth", &depth, "origin", &pr.base_sha],
        config.job_timeout,
        true,
    )
    .await?;
    let changed = if base_fetch.status.success() {
        let diff = run_git(
            config,
            log_path,
            &repo_dir,
            &["diff", "--name-only", &pr.base_sha, &pr.head_sha],
            Duration::from_secs(60),
            false,
        )
        .await?;
        if diff.status.success() {
            let paths: Vec<String> = diff
                .stdout
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect();
            append_log(
                log_path,
                &format!("PR diff produced {} changed paths\n", paths.len()),
                config.max_log_bytes,
            )
            .await;
            Some(paths)
        } else {
            append_log(
                log_path,
                &format!(
                    "PR diff base..head failed: {} (will scan whole tree)\n",
                    diff.stderr
                ),
                config.max_log_bytes,
            )
            .await;
            None
        }
    } else {
        append_log(
            log_path,
            &format!(
                "git fetch base_sha failed: {} (will scan whole tree)\n",
                base_fetch.stderr
            ),
            config.max_log_bytes,
        )
        .await;
        None
    };

    Ok((repo_dir, changed))
}

struct JobOutcome {
    findings: Vec<Finding>,
    files_scanned: usize,
    z3_queries: u64,
    changed_paths: Option<Vec<String>>,
}

async fn execute_job(state: &AppState, job: &JobRecord) -> Result<JobOutcome, String> {
    let config = state.config.as_ref();
    let request = &job.request;
    let job_dir = config.work_root.join(&job.id);
    let log_path = PathBuf::from(&job.log_path);

    fs::create_dir_all(&job_dir)
        .await
        .map_err(|error| format!("failed to create job dir: {error}"))?;
    append_log(
        &log_path,
        &format!(
            "{SERVICE_NAME} starting job={} repo={} inline={} pr={}\n",
            job.id,
            request.repo_url.as_deref().unwrap_or("<none>"),
            request.inline_source.is_some(),
            request
                .pull_request
                .as_ref()
                .map(|p| format!("{}/{}#{}", p.owner, p.repo, p.number))
                .unwrap_or_else(|| "<none>".to_string())
        ),
        config.max_log_bytes,
    )
    .await;

    let languages_filter: Option<HashSet<String>> = request.languages.as_ref().map(|langs| {
        langs
            .iter()
            .map(|l| l.trim_start_matches('.').to_ascii_lowercase())
            .collect()
    });
    let mut path_filter: Option<Vec<PathBuf>> = match request.paths.as_ref() {
        Some(paths) => {
            let mut clean = Vec::new();
            for path in paths {
                clean.push(validate_relative_path("paths[]", path)?);
            }
            Some(clean)
        }
        None => None,
    };

    let z3_calls = AtomicU64::new(0);
    let z3_failures = AtomicU64::new(0);
    let heuristics_enabled = request.heuristics.unwrap_or(true);

    let (findings, files_scanned, changed_paths) =
        if let Some(source) = request.inline_source.as_deref() {
            let file_label = request
                .inline_filename
                .as_deref()
                .unwrap_or("inline.txt")
                .to_string();
            let ctx = VerifyContext { config };
            let parsed = parse_annotations(&file_label, source);
            let mut decls_lookup = HashMap::new();
            for block in &parsed.blocks {
                for decl in &block.decls {
                    decls_lookup.insert(decl.name.clone(), decl.sort.clone());
                }
            }
            let mut findings = Vec::new();
            for block in &parsed.blocks {
                let mut block_findings = verify_block(&ctx, block, &z3_calls, &z3_failures).await;
                findings.append(&mut block_findings);
            }
            if heuristics_enabled && !decls_lookup.is_empty() {
                let mut h =
                    heuristic_checks(&ctx, &parsed, &decls_lookup, &z3_calls, &z3_failures).await;
                findings.append(&mut h);
            }
            (findings, 1usize, None)
        } else if let Some(pr) = request.pull_request.as_ref() {
            let (repo_dir, changed) = clone_for_pr(config, &log_path, &job_dir, pr).await?;
            if config.pr_diff_only && path_filter.is_none() {
                if let Some(changed_paths) = changed.as_ref() {
                    let cleaned: Vec<PathBuf> = changed_paths
                        .iter()
                        .filter_map(|p| validate_relative_path("changed_paths[]", p).ok())
                        .collect();
                    if !cleaned.is_empty() {
                        path_filter = Some(cleaned);
                    }
                }
            }
            let (findings, files_scanned) = analyze_tree(
                state,
                &repo_dir,
                &languages_filter,
                &path_filter,
                heuristics_enabled,
                &log_path,
                &z3_calls,
                &z3_failures,
            )
            .await;
            (findings, files_scanned, changed)
        } else {
            let repo_url = request
                .repo_url
                .as_deref()
                .ok_or_else(|| "repoUrl missing".to_string())?;
            let git_ref = clean_optional(request.git_ref.as_deref());
            let repo_dir =
                clone_repo(config, &log_path, &job_dir, repo_url, git_ref.as_deref()).await?;
            let (findings, files_scanned) = analyze_tree(
                state,
                &repo_dir,
                &languages_filter,
                &path_filter,
                heuristics_enabled,
                &log_path,
                &z3_calls,
                &z3_failures,
            )
            .await;
            (findings, files_scanned, None)
        };

    let z3_calls_final = z3_calls.load(Ordering::Relaxed);
    let z3_failures_final = z3_failures.load(Ordering::Relaxed);
    state
        .counters
        .z3_calls
        .fetch_add(z3_calls_final, Ordering::Relaxed);
    state
        .counters
        .z3_failures
        .fetch_add(z3_failures_final, Ordering::Relaxed);

    append_log(
        &log_path,
        &format!(
            "{SERVICE_NAME} completed job={} findings={} files={} z3_calls={}\n",
            job.id,
            findings.len(),
            files_scanned,
            z3_calls_final
        ),
        config.max_log_bytes,
    )
    .await;

    Ok(JobOutcome {
        findings,
        files_scanned,
        z3_queries: z3_calls_final,
        changed_paths,
    })
}

pub(crate) async fn run_job(state: AppState, id: String) {
    let permit = match state.semaphore.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(error) => {
            update_job(&state, &id, |job| {
                job.status = JobStatus::Failed;
                job.finished_at_ms = Some(now_ms());
                job.error = Some(format!("queue is closed: {error}"));
            })
            .await;
            return;
        }
    };
    state.counters.running.fetch_add(1, Ordering::Relaxed);
    update_job(&state, &id, |job| {
        job.status = JobStatus::Running;
        job.started_at_ms = Some(now_ms());
    })
    .await;
    let job_snapshot = {
        let jobs = state.jobs.read().await;
        jobs.get(&id).cloned()
    };
    let result = match job_snapshot.as_ref() {
        Some(job) => execute_job(&state, job).await,
        None => Err("job disappeared before execution".to_string()),
    };
    state.counters.running.fetch_sub(1, Ordering::Relaxed);
    drop(permit);

    match result {
        Ok(outcome) => {
            state.counters.succeeded.fetch_add(1, Ordering::Relaxed);
            state
                .counters
                .findings_total
                .fetch_add(outcome.findings.len() as u64, Ordering::Relaxed);

            let pr = job_snapshot.as_ref().and_then(|j| j.pull_request.clone());
            let log_path = job_snapshot.as_ref().map(|j| PathBuf::from(&j.log_path));

            let comment_status = if let Some(pr) = pr.as_ref() {
                if state.config.pr_comment_enabled && state.config.github_api_token.is_some() {
                    let body = render_pr_comment_body(
                        pr,
                        &outcome.findings,
                        &id,
                        outcome.files_scanned,
                        outcome.z3_queries,
                        outcome.changed_paths.as_ref().map(|c| c.len()),
                        &state.config,
                    );
                    match post_pr_comment(&state, pr, &body).await {
                        Ok(()) => {
                            state
                                .counters
                                .pr_comments_posted
                                .fetch_add(1, Ordering::Relaxed);
                            if let Some(log) = log_path.as_ref() {
                                append_log(
                                    log,
                                    &format!(
                                        "posted PR comment to {}/{}#{}\n",
                                        pr.owner, pr.repo, pr.number
                                    ),
                                    state.config.max_log_bytes,
                                )
                                .await;
                            }
                            Some("posted".to_string())
                        }
                        Err(error) => {
                            state
                                .counters
                                .pr_comments_failed
                                .fetch_add(1, Ordering::Relaxed);
                            if let Some(log) = log_path.as_ref() {
                                append_log(
                                    log,
                                    &format!("PR comment failed: {error}\n"),
                                    state.config.max_log_bytes,
                                )
                                .await;
                            }
                            Some(format!("failed: {error}"))
                        }
                    }
                } else {
                    Some("disabled".to_string())
                }
            } else {
                None
            };

            update_job(&state, &id, |job| {
                job.status = JobStatus::Succeeded;
                job.finished_at_ms = Some(now_ms());
                job.error = None;
                job.findings_count = outcome.findings.len();
                job.findings = outcome.findings;
                job.files_scanned = outcome.files_scanned;
                job.z3_queries = outcome.z3_queries;
                job.changed_paths = outcome.changed_paths;
                job.pr_comment_status = comment_status;
            })
            .await;
        }
        Err(error) => {
            state.counters.failed.fetch_add(1, Ordering::Relaxed);
            update_job(&state, &id, |job| {
                job.status = JobStatus::Failed;
                job.finished_at_ms = Some(now_ms());
                job.error = Some(error);
            })
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_sha_like_matches_short_and_long_hex() {
        assert!(is_sha_like("deadbeef"));
        assert!(is_sha_like("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"));
        assert!(!is_sha_like("not-a-sha"));
        assert!(!is_sha_like("abc"));
        assert!(!is_sha_like(""));
    }
}
