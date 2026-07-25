use std::{collections::HashMap, io::BufReader};

use serde_json::{json, Value};

use crate::shared::{
    agent_tasks_admin_user_id, data_config, first_env, infer_repo_display_name, json_i32,
    json_i64, json_string, normalize_base_branch, normalize_context_mode, normalize_repo_provider,
    normalize_repo_url, normalized_repo_config, now_ms, postgres_database_url,
    public_data_source_error, row_i32, row_i64, row_opt_string, row_string,
};
use crate::state::{runtime_snapshot, summarize};
use crate::types::{
    AgentBreadcrumbIngestRequest, AgentBreadcrumbRow, AgentEventIngestRequest, AgentEventRow,
    AgentFeedbackRequest, AgentTaskRow, AgentThreadRow, AgentsSnapshot, DispatchTaskRequest,
    ExistingTaskDispatch, KnownGitRepoRow, ThreadRepoConfig,
};

pub(crate) fn add_rds_root_certificates(root_store: &mut rustls::RootCertStore) -> Result<(), String> {
    let mut reader = BufReader::new(&include_bytes!("../certs/rds-us-east-1-bundle.pem")[..]);
    let mut added = 0usize;

    for cert in rustls_pemfile::certs(&mut reader) {
        let cert = cert.map_err(|error| format!("failed to parse RDS CA certificate: {error}"))?;
        if root_store.add(cert).is_ok() {
            added += 1;
        }
    }

    if added == 0 {
        return Err("no RDS CA certificates loaded".to_string());
    }

    Ok(())
}

pub(crate) async fn fetch_agents_snapshot(limit: i64) -> AgentsSnapshot {
    let config = data_config();
    let mut errors = Vec::new();

    if config.postgres_configured {
        match fetch_agents_from_postgres(limit).await {
            Ok((threads, tasks)) => {
                return AgentsSnapshot {
                    ok: true,
                    source: if config.rds_configured {
                        "rds-postgres".to_string()
                    } else {
                        "postgres".to_string()
                    },
                    generated_at_ms: now_ms(),
                    summary: summarize(&threads, &tasks),
                    threads,
                    tasks,
                    errors,
                    config,
                };
            }
            Err(error) => {
                tracing::error!("agent tasks postgres data source error: {error}");
                errors.push(public_data_source_error("postgres"));
            }
        }
    }

    if config.supabase_configured {
        match fetch_agents_from_supabase(limit).await {
            Ok((threads, tasks)) => {
                return AgentsSnapshot {
                    ok: true,
                    source: "supabase".to_string(),
                    generated_at_ms: now_ms(),
                    summary: summarize(&threads, &tasks),
                    threads,
                    tasks,
                    errors,
                    config,
                };
            }
            Err(error) => {
                tracing::error!("agent tasks supabase data source error: {error}");
                errors.push(public_data_source_error("supabase"));
            }
        }
    }

    if !config.postgres_configured && !config.supabase_configured {
        errors.push(
            "agent tasks data source is not configured; showing runtime memory only".to_string(),
        );
    }

    runtime_snapshot(limit, config, errors)
}

pub(crate) async fn connect_postgres() -> Result<tokio_postgres::Client, String> {
    let database_url = postgres_database_url()
        .ok_or_else(|| "postgres database URL not configured".to_string())?;
    connect_postgres_with_url(&database_url).await
}

pub(crate) async fn connect_postgres_with_url(database_url: &str) -> Result<tokio_postgres::Client, String> {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    add_rds_root_certificates(&mut root_store)?;
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
    let (client, connection) = tokio_postgres::connect(database_url, tls)
        .await
        .map_err(|error| error.to_string())?;

    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::error!("agent tasks postgres connection error: {error}");
        }
    });
    Ok(client)
}

pub(crate) async fn fetch_agents_from_postgres(
    limit: i64,
) -> Result<(Vec<AgentThreadRow>, Vec<AgentTaskRow>), String> {
    let client = connect_postgres().await?;

    let thread_rows = client
        .query(
            r#"
            select
              th.id::text as id,
              th.title as title,
              th.repo as repo,
              th.base_branch as base_branch,
              to_char(th.archived_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as archived_at,
              to_char(th.created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at,
              to_char(th.updated_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as updated_at,
              count(t.id)::bigint as task_count,
              count(t.id) filter (
                where t.status in ('queued', 'running', 'streaming')
                  and t.finished_at is null
              )::bigint as active_task_count,
              to_char(max(t.created_at) at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as latest_task_at
            from agent_remote_dev_threads th
            left join agent_remote_dev_tasks t
              on t.thread_id = th.id and t.is_soft_deleted = false
            where th.is_soft_deleted = false
            group by th.id, th.title, th.repo, th.base_branch, th.archived_at, th.created_at, th.updated_at
            order by coalesce(max(t.created_at), th.updated_at, th.created_at) desc
            limit $1
            "#,
            &[&limit],
        )
        .await
        .map_err(|error| error.to_string())?;

    let task_rows = client
        .query(
            r#"
            select
              t.id::text as id,
              t.thread_id::text as thread_id,
              th.title as thread_title,
              t.prompt as prompt,
              case
                when t.status in ('pr_open', 'pr_merged', 'pr_closed') then t.status
                when t.finished_at is not null and coalesce(t.exit_reason, 'completed') = 'completed' then 'done'
                when t.finished_at is not null and t.exit_reason = 'cancelled' then 'cancelled'
                when t.finished_at is not null then 'failed'
                when le.event_kind = 'done' and coalesce(le.payload->>'exitReason', 'completed') = 'completed' then 'done'
                when le.event_kind = 'done' and le.payload->>'exitReason' = 'cancelled' then 'cancelled'
                when le.event_kind = 'done' then 'failed'
                else t.status
              end as status,
              t.branch as branch,
              t.pr_url as pr_url,
              t.pr_state as pr_state,
              t.exit_reason as exit_reason,
              t.error_message as error_message,
              to_char(t.started_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as started_at,
              to_char(t.finished_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as finished_at,
              to_char(t.created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at,
              to_char(t.updated_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as updated_at,
              t.last_event_seq as last_event_seq,
              coalesce(e.event_count, 0)::bigint as event_count,
              le.event_kind as latest_event_kind,
              left(le.payload::text, 1200) as latest_payload
            from agent_remote_dev_tasks t
            left join agent_remote_dev_threads th on th.id = t.thread_id
            left join lateral (
              select count(*)::bigint as event_count
              from agent_remote_dev_events ev
              where ev.task_id = t.id
            ) e on true
            left join lateral (
              select ev.event_kind, ev.payload
              from agent_remote_dev_events ev
              where ev.task_id = t.id
              order by ev.seq desc
              limit 1
            ) le on true
            where t.is_soft_deleted = false
            order by t.created_at desc
            limit $1
            "#,
            &[&limit],
        )
        .await
        .map_err(|error| error.to_string())?;

    let threads = thread_rows
        .iter()
        .map(|row| AgentThreadRow {
            id: row_string(row, "id"),
            title: row_string(row, "title"),
            repo: row_string(row, "repo"),
            base_branch: row_string(row, "base_branch"),
            archived_at: row_opt_string(row, "archived_at"),
            created_at: row_opt_string(row, "created_at"),
            updated_at: row_opt_string(row, "updated_at"),
            task_count: row_i64(row, "task_count"),
            active_task_count: row_i64(row, "active_task_count"),
            latest_task_at: row_opt_string(row, "latest_task_at"),
        })
        .collect();

    let tasks = task_rows
        .iter()
        .map(|row| AgentTaskRow {
            id: row_string(row, "id"),
            thread_id: row_string(row, "thread_id"),
            thread_title: row_opt_string(row, "thread_title"),
            prompt: row_string(row, "prompt"),
            status: row_string(row, "status"),
            branch: row_opt_string(row, "branch"),
            pr_url: row_opt_string(row, "pr_url"),
            pr_state: row_opt_string(row, "pr_state"),
            exit_reason: row_opt_string(row, "exit_reason"),
            error_message: row_opt_string(row, "error_message"),
            started_at: row_opt_string(row, "started_at"),
            finished_at: row_opt_string(row, "finished_at"),
            created_at: row_opt_string(row, "created_at"),
            updated_at: row_opt_string(row, "updated_at"),
            last_event_seq: row_i32(row, "last_event_seq"),
            event_count: row_i64(row, "event_count"),
            latest_event_kind: row_opt_string(row, "latest_event_kind"),
            latest_payload: row_opt_string(row, "latest_payload"),
        })
        .collect();

    Ok((threads, tasks))
}

pub(crate) async fn fetch_known_git_repos_from_postgres(limit: i64) -> Result<Vec<KnownGitRepoRow>, String> {
    let client = connect_postgres().await?;
    let rows = client
        .query(
            r#"
            select
              id::text as id,
              repo_url,
              display_name,
              provider,
              default_branch,
              status,
              to_char(last_verified_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as last_verified_at,
              to_char(created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at,
              to_char(updated_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as updated_at
            from known_git_repos
            where is_soft_deleted = false
            order by updated_at desc
            limit $1
            "#,
            &[&limit],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(rows
        .iter()
        .map(|row| KnownGitRepoRow {
            id: row_string(row, "id"),
            repo_url: row_string(row, "repo_url"),
            display_name: row_string(row, "display_name"),
            provider: row_string(row, "provider"),
            default_branch: row_string(row, "default_branch"),
            status: row_string(row, "status"),
            last_verified_at: row_opt_string(row, "last_verified_at"),
            created_at: row_opt_string(row, "created_at"),
            updated_at: row_opt_string(row, "updated_at"),
        })
        .collect())
}

pub(crate) async fn upsert_known_git_repo_to_postgres(
    repo_url: &str,
    display_name: Option<&str>,
    provider: Option<&str>,
    default_branch: Option<&str>,
) -> Result<KnownGitRepoRow, String> {
    let client = connect_postgres().await?;
    let admin_user_id = agent_tasks_admin_user_id();
    let repo_url = normalize_repo_url(repo_url)?;
    let display_name = display_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(200).collect::<String>())
        .unwrap_or_else(|| infer_repo_display_name(&repo_url));
    let provider = provider.map(str::trim).filter(|value| !value.is_empty());
    let provider = normalize_repo_provider(provider, &repo_url)?;
    let default_branch = normalize_base_branch(default_branch)?;

    let row = client
        .query_one(
            r#"
            insert into known_git_repos
              (repo_url, display_name, provider, default_branch, status, is_soft_deleted, created_at, updated_at, created_by, updated_by)
            values
              ($1, $2, $3, $4, 'active', false, now(), now(), $5::text::uuid, $5::text::uuid)
            on conflict (repo_url) where is_soft_deleted = false do update set
              display_name = excluded.display_name,
              provider = excluded.provider,
              default_branch = excluded.default_branch,
              status = 'active',
              updated_by = excluded.updated_by,
              updated_at = now()
            returning
              id::text as id,
              repo_url,
              display_name,
              provider,
              default_branch,
              status,
              to_char(last_verified_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as last_verified_at,
              to_char(created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at,
              to_char(updated_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as updated_at
            "#,
            &[&repo_url, &display_name, &provider, &default_branch, &admin_user_id],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(KnownGitRepoRow {
        id: row_string(&row, "id"),
        repo_url: row_string(&row, "repo_url"),
        display_name: row_string(&row, "display_name"),
        provider: row_string(&row, "provider"),
        default_branch: row_string(&row, "default_branch"),
        status: row_string(&row, "status"),
        last_verified_at: row_opt_string(&row, "last_verified_at"),
        created_at: row_opt_string(&row, "created_at"),
        updated_at: row_opt_string(&row, "updated_at"),
    })
}

pub(crate) async fn fetch_thread_repo_config_from_postgres(
    thread_id: &str,
) -> Result<Option<ThreadRepoConfig>, String> {
    if postgres_database_url().is_none() {
        return Ok(None);
    }
    let client = connect_postgres().await?;
    let row = client
        .query_opt(
            r#"
            select repo, base_branch, title as thread_title
            from agent_remote_dev_threads
            where id = $1::text::uuid
              and is_soft_deleted = false
            limit 1
            "#,
            &[&thread_id],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(row.map(|row| ThreadRepoConfig {
        repo: row_string(&row, "repo"),
        base_branch: row_string(&row, "base_branch"),
        thread_title: row_opt_string(&row, "thread_title"),
    }))
}

pub(crate) async fn persist_runtime_task_to_postgres(
    request: &DispatchTaskRequest,
    branch: Option<&str>,
    status: &str,
) -> Result<(), String> {
    let admin_user_id = agent_tasks_admin_user_id().ok_or_else(|| {
        "AGENT_TASKS_ADMIN_USER_ID or REMOTE_DEV_ADMIN_USER_ID is not configured".to_string()
    })?;
    let repo_config = normalized_repo_config(request)?;
    let known_repo = upsert_known_git_repo_to_postgres(
        &repo_config.repo,
        None,
        None,
        Some(&repo_config.base_branch),
    )
    .await?;
    let client = connect_postgres().await?;
    let title = request
        .thread_title
        .clone()
        .unwrap_or_else(|| request.prompt.chars().take(80).collect::<String>());
    let context_ids = request.context_ids.clone().unwrap_or_default();
    let context_mode = normalize_context_mode(request.context_mode.as_deref(), context_ids.len());
    let task_meta = json!({
        "contextMode": context_mode,
        "contextIds": context_ids,
    });

    let affected_thread_rows = client
        .execute(
            r#"
            insert into agent_remote_dev_threads
              (id, user_id, known_git_repo_id, title, repo, base_branch, is_soft_deleted, created_at, updated_at, created_by, updated_by)
            values
              ($1::text::uuid, $2::text::uuid, $3::text::uuid, $4, $5, $6, false, now(), now(), $2::text::uuid, $2::text::uuid)
            on conflict (id) do update set
              title = coalesce(agent_remote_dev_threads.title, excluded.title),
              known_git_repo_id = coalesce(agent_remote_dev_threads.known_git_repo_id, excluded.known_git_repo_id),
              updated_by = excluded.updated_by,
              updated_at = now()
            where agent_remote_dev_threads.repo = excluded.repo
              and agent_remote_dev_threads.base_branch = excluded.base_branch
            "#,
            &[
                &request.thread_id,
                &admin_user_id,
                &known_repo.id,
                &title,
                &repo_config.repo,
                &repo_config.base_branch,
            ],
        )
        .await
        .map_err(|error| error.to_string())?;
    if affected_thread_rows == 0 {
        return Err("thread already exists with a different repo or baseBranch".to_string());
    }

    client
        .execute(
            r#"
            insert into agent_remote_dev_tasks
              (id, thread_id, user_id, docker_task_id, prompt, status, branch, last_event_seq, meta, is_soft_deleted, started_at, created_at, updated_at, created_by, updated_by)
            values
              ($1::text::uuid, $2::text::uuid, $3::text::uuid, $1::text::uuid, $4, $6, $5, -1, $7, false, now(), now(), now(), $3::text::uuid, $3::text::uuid)
            on conflict (id) do update set
              prompt = agent_remote_dev_tasks.prompt,
              status = case
                when agent_remote_dev_tasks.status in ('pr_open', 'pr_merged', 'pr_closed')
                then agent_remote_dev_tasks.status
                when agent_remote_dev_tasks.finished_at is not null
                  and excluded.status in ('queued', 'running', 'streaming')
                then case
                  when coalesce(agent_remote_dev_tasks.exit_reason, 'completed') = 'completed' then 'done'
                  when agent_remote_dev_tasks.exit_reason = 'cancelled' then 'cancelled'
                  else 'failed'
                end
                when agent_remote_dev_tasks.status in ('done', 'cancelled', 'failed', 'pr_open', 'pr_merged', 'pr_closed')
                  and excluded.status in ('queued', 'running', 'streaming')
                then agent_remote_dev_tasks.status
                else excluded.status
              end,
              branch = coalesce(excluded.branch, agent_remote_dev_tasks.branch),
              meta = agent_remote_dev_tasks.meta || excluded.meta,
              updated_by = excluded.updated_by,
              updated_at = now()
            "#,
            &[
                &request.task_id,
                &request.thread_id,
                &admin_user_id,
                &request.prompt,
                &branch,
                &status,
                &task_meta,
            ],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(())
}

pub(crate) async fn fetch_existing_task_dispatch_from_postgres(
    task_id: &str,
) -> Result<Option<ExistingTaskDispatch>, String> {
    let client = connect_postgres().await?;
    let row = client
        .query_opt(
            r#"
            select
              thread_id::text as thread_id,
              prompt
            from agent_remote_dev_tasks
            where id = $1::text::uuid
              and is_soft_deleted = false
            "#,
            &[&task_id],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(row.map(|row| ExistingTaskDispatch {
        thread_id: row_string(&row, "thread_id"),
        prompt: row_string(&row, "prompt"),
    }))
}

pub(crate) fn task_status_from_exit_reason(exit_reason: &str) -> &'static str {
    match exit_reason {
        "completed" => "done",
        "cancelled" => "cancelled",
        _ => "failed",
    }
}

pub(crate) async fn persist_agent_event_to_postgres(
    request: &AgentEventIngestRequest,
    event_kind: &str,
) -> Result<(), String> {
    let client = connect_postgres().await?;
    client
        .execute(
            r#"
            insert into agent_remote_dev_events
              (task_id, thread_id, seq, event_kind, payload, created_at)
            values
              ($1::text::uuid, $2::text::uuid, $3, $4, $5, now())
            on conflict (task_id, seq) do update set
              thread_id = coalesce(excluded.thread_id, agent_remote_dev_events.thread_id),
              event_kind = excluded.event_kind,
              payload = excluded.payload
            "#,
            &[
                &request.task_id,
                &request.thread_id,
                &request.seq,
                &event_kind,
                &request.event,
            ],
        )
        .await
        .map_err(|error| error.to_string())?;

    client
        .execute(
            r#"
            update agent_remote_dev_tasks
            set
              last_event_seq = greatest(last_event_seq, $2),
              updated_at = now()
            where id = $1::text::uuid
              and $2 > last_event_seq
            "#,
            &[&request.task_id, &request.seq],
        )
        .await
        .map_err(|error| error.to_string())?;

    if event_kind == "done" {
        let exit_reason =
            json_string(&request.event, "exitReason").unwrap_or_else(|| "failed".to_string());
        let status = task_status_from_exit_reason(&exit_reason);
        let branch = json_string(&request.event, "branch");
        let pr_url = json_string(&request.event, "prUrl");
        let error_message = json_string(&request.event, "errorMessage");
        client
            .execute(
                r#"
                update agent_remote_dev_tasks
                set
                  status = $2,
                  branch = coalesce($3, branch),
                  pr_url = coalesce($4, pr_url),
                  exit_reason = $5,
                  error_message = $6,
                  finished_at = now(),
                  updated_at = now()
                where id = $1::text::uuid
                "#,
                &[
                    &request.task_id,
                    &status,
                    &branch,
                    &pr_url,
                    &exit_reason,
                    &error_message,
                ],
            )
            .await
            .map_err(|error| error.to_string())?;
    }

    if event_kind == "pr_open" {
        let branch = json_string(&request.event, "branch");
        let pr_url = json_string(&request.event, "prUrl");
        client
            .execute(
                r#"
                update agent_remote_dev_tasks
                set
                  status = 'pr_open',
                  branch = coalesce($2, branch),
                  pr_url = coalesce($3, pr_url),
                  pr_state = 'draft',
                  updated_at = now()
                where id = $1::text::uuid
                "#,
                &[&request.task_id, &branch, &pr_url],
            )
            .await
            .map_err(|error| error.to_string())?;
    }

    Ok(())
}

pub(crate) async fn persist_agent_breadcrumb_to_postgres(
    request: &AgentBreadcrumbIngestRequest,
) -> Result<AgentBreadcrumbRow, String> {
    let payload = request
        .payload
        .clone()
        .unwrap_or_else(|| Value::Object(Default::default()));
    if !payload.is_object() {
        return Err("payload must be a JSON object".to_string());
    }
    let client = connect_postgres().await?;
    let row = client
        .query_one(
            r#"
            insert into agent_remote_dev_breadcrumbs
              (thread_id, task_id, kind, payload, emitted_at, pod_name, branch, provider)
            values
              ($1::text::uuid, $2::text::uuid, $3, $4, now(), $5, $6, $7)
            returning
              id,
              thread_id::text as thread_id,
              task_id::text as task_id,
              kind,
              payload,
              to_char(emitted_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as emitted_at,
              pod_name,
              branch,
              provider
            "#,
            &[
                &request.thread_id,
                &request.task_id,
                &request.kind,
                &payload,
                &request.pod_name,
                &request.branch,
                &request.provider,
            ],
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(AgentBreadcrumbRow {
        id: row
            .try_get::<_, i64>("id")
            .map_err(|error| error.to_string())?,
        thread_id: row_string(&row, "thread_id"),
        task_id: row_opt_string(&row, "task_id"),
        kind: row_string(&row, "kind"),
        payload: row
            .try_get::<_, Value>("payload")
            .unwrap_or(Value::Object(Default::default())),
        emitted_at: row_string(&row, "emitted_at"),
        pod_name: row_opt_string(&row, "pod_name"),
        branch: row_opt_string(&row, "branch"),
        provider: row_opt_string(&row, "provider"),
    })
}

pub(crate) async fn fetch_agent_breadcrumb_tail_from_postgres(
    thread_id: &str,
    limit: i64,
    exclude_task_id: Option<&str>,
) -> Result<Vec<AgentBreadcrumbRow>, String> {
    let client = connect_postgres().await?;
    let rows = match exclude_task_id {
        Some(task_id) => client
            .query(
                r#"
                select
                  id,
                  thread_id::text as thread_id,
                  task_id::text as task_id,
                  kind,
                  payload,
                  to_char(emitted_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as emitted_at,
                  pod_name,
                  branch,
                  provider
                from agent_remote_dev_breadcrumbs
                where thread_id = $1::text::uuid
                  and (task_id is null or task_id <> $2::text::uuid)
                order by emitted_at desc, id desc
                limit $3
                "#,
                &[&thread_id, &task_id, &limit],
            )
            .await,
        None => client
            .query(
                r#"
                select
                  id,
                  thread_id::text as thread_id,
                  task_id::text as task_id,
                  kind,
                  payload,
                  to_char(emitted_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as emitted_at,
                  pod_name,
                  branch,
                  provider
                from agent_remote_dev_breadcrumbs
                where thread_id = $1::text::uuid
                order by emitted_at desc, id desc
                limit $2
                "#,
                &[&thread_id, &limit],
            )
            .await,
    }
    .map_err(|error| error.to_string())?;

    Ok(rows
        .iter()
        .map(|row| AgentBreadcrumbRow {
            id: row.try_get::<_, i64>("id").unwrap_or(0),
            thread_id: row_string(row, "thread_id"),
            task_id: row_opt_string(row, "task_id"),
            kind: row_string(row, "kind"),
            payload: row
                .try_get::<_, Value>("payload")
                .unwrap_or(Value::Object(Default::default())),
            emitted_at: row_string(row, "emitted_at"),
            pod_name: row_opt_string(row, "pod_name"),
            branch: row_opt_string(row, "branch"),
            provider: row_opt_string(row, "provider"),
        })
        .collect())
}

pub(crate) async fn fetch_agent_events_from_postgres(
    task_id: &str,
    limit: i64,
) -> Result<Vec<AgentEventRow>, String> {
    let client = connect_postgres().await?;
    let event_rows = client
        .query(
            r#"
            select
              ev.task_id::text as task_id,
              ev.seq as seq,
              ev.event_kind as event_kind,
              ev.payload as payload,
              to_char(ev.created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at
            from (
              select task_id, seq, event_kind, payload, created_at
              from agent_remote_dev_events
              where task_id = $1::text::uuid
              order by seq desc
              limit $2
            ) ev
            order by ev.seq asc
            "#,
            &[&task_id, &limit],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(event_rows
        .iter()
        .map(|row| AgentEventRow {
            task_id: row_string(row, "task_id"),
            seq: row_i32(row, "seq"),
            event_kind: row_string(row, "event_kind"),
            payload: row.get("payload"),
            created_at: row_opt_string(row, "created_at"),
        })
        .collect())
}

pub(crate) async fn persist_feedback_event_to_postgres(
    task_id: &str,
    request: &AgentFeedbackRequest,
) -> Result<AgentEventRow, String> {
    let client = connect_postgres().await?;
    let vote = request.vote.trim().to_lowercase();
    let seq_row = client
        .query_one(
            r#"
            select coalesce(max(seq), -1) + 1 as next_seq
            from agent_remote_dev_events
            where task_id = $1::text::uuid
            "#,
            &[&task_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    let seq: i32 = seq_row.get("next_seq");
    let payload = json!({
        "kind": "feedback",
        "vote": vote,
        "targetSeq": request.target_seq,
        "note": request.note,
        "source": "agents-threads-ui",
        "createdAtMs": now_ms(),
    });

    let event_row = client
        .query_one(
            r#"
            insert into agent_remote_dev_events
              (task_id, seq, event_kind, payload, created_at)
            values
              ($1::text::uuid, $2, 'feedback', $3, now())
            returning
              task_id::text as task_id,
              seq,
              event_kind,
              payload,
              to_char(created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at
            "#,
            &[&task_id, &seq, &payload],
        )
        .await
        .map_err(|error| error.to_string())?;

    client
        .execute(
            r#"
            update agent_remote_dev_tasks
            set
              last_event_seq = greatest(last_event_seq, $2),
              updated_at = now()
            where id = $1::text::uuid
            "#,
            &[&task_id, &seq],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(AgentEventRow {
        task_id: row_string(&event_row, "task_id"),
        seq: row_i32(&event_row, "seq"),
        event_kind: row_string(&event_row, "event_kind"),
        payload: event_row.get("payload"),
        created_at: row_opt_string(&event_row, "created_at"),
    })
}

pub(crate) async fn fetch_thread_context_from_postgres(
    thread_id: &str,
    limit: i64,
) -> Result<Vec<AgentTaskRow>, String> {
    let client = connect_postgres().await?;
    let task_rows = client
        .query(
            r#"
            select
              t.id::text as id,
              t.thread_id::text as thread_id,
              th.title as thread_title,
              t.prompt as prompt,
              case
                when t.status in ('pr_open', 'pr_merged', 'pr_closed') then t.status
                when t.finished_at is not null and coalesce(t.exit_reason, 'completed') = 'completed' then 'done'
                when t.finished_at is not null and t.exit_reason = 'cancelled' then 'cancelled'
                when t.finished_at is not null then 'failed'
                when le.event_kind = 'done' and coalesce(le.payload->>'exitReason', 'completed') = 'completed' then 'done'
                when le.event_kind = 'done' and le.payload->>'exitReason' = 'cancelled' then 'cancelled'
                when le.event_kind = 'done' then 'failed'
                else t.status
              end as status,
              t.branch as branch,
              t.pr_url as pr_url,
              t.pr_state as pr_state,
              t.exit_reason as exit_reason,
              t.error_message as error_message,
              to_char(t.started_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as started_at,
              to_char(t.finished_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as finished_at,
              to_char(t.created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at,
              to_char(t.updated_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as updated_at,
              t.last_event_seq as last_event_seq,
              coalesce(e.event_count, 0)::bigint as event_count,
              le.event_kind as latest_event_kind,
              left(le.payload::text, 1200) as latest_payload
            from agent_remote_dev_tasks t
            left join agent_remote_dev_threads th on th.id = t.thread_id
            left join lateral (
              select count(*)::bigint as event_count
              from agent_remote_dev_events ev
              where ev.task_id = t.id
            ) e on true
            left join lateral (
              select ev.event_kind, ev.payload
              from agent_remote_dev_events ev
              where ev.task_id = t.id
              order by ev.seq desc
              limit 1
            ) le on true
            where t.thread_id = $1::text::uuid
              and t.is_soft_deleted = false
            order by t.created_at desc
            limit $2
            "#,
            &[&thread_id, &limit],
        )
        .await
        .map_err(|error| error.to_string())?;

    let mut tasks = task_rows
        .iter()
        .map(|row| AgentTaskRow {
            id: row_string(row, "id"),
            thread_id: row_string(row, "thread_id"),
            thread_title: row_opt_string(row, "thread_title"),
            prompt: row_string(row, "prompt"),
            status: row_string(row, "status"),
            branch: row_opt_string(row, "branch"),
            pr_url: row_opt_string(row, "pr_url"),
            pr_state: row_opt_string(row, "pr_state"),
            exit_reason: row_opt_string(row, "exit_reason"),
            error_message: row_opt_string(row, "error_message"),
            started_at: row_opt_string(row, "started_at"),
            finished_at: row_opt_string(row, "finished_at"),
            created_at: row_opt_string(row, "created_at"),
            updated_at: row_opt_string(row, "updated_at"),
            last_event_seq: row_i32(row, "last_event_seq"),
            event_count: row_i64(row, "event_count"),
            latest_event_kind: row_opt_string(row, "latest_event_kind"),
            latest_payload: row_opt_string(row, "latest_payload"),
        })
        .collect::<Vec<_>>();
    tasks.reverse();
    Ok(tasks)
}

pub(crate) async fn fetch_agents_from_supabase(
    limit: i64,
) -> Result<(Vec<AgentThreadRow>, Vec<AgentTaskRow>), String> {
    let supabase_url = first_env(&["SUPABASE_URL", "NEXT_PUBLIC_SUPABASE_URL"])
        .ok_or_else(|| "SUPABASE_URL not configured".to_string())?;
    let supabase_key = first_env(&["SUPABASE_SERVICE_ROLE_KEY", "SUPABASE_KEY"])
        .ok_or_else(|| "SUPABASE_SERVICE_ROLE_KEY not configured".to_string())?;
    let base = supabase_url.trim_end_matches('/');
    let http = reqwest::Client::new();

    let threads_url = format!(
        "{base}/rest/v1/agent_remote_dev_threads?select=id,title,repo,base_branch,archived_at,created_at,updated_at&is_soft_deleted=eq.false&order=updated_at.desc&limit={limit}"
    );
    let tasks_url = format!(
        "{base}/rest/v1/agent_remote_dev_tasks?select=id,thread_id,prompt,status,branch,pr_url,pr_state,exit_reason,error_message,started_at,finished_at,created_at,updated_at,last_event_seq&is_soft_deleted=eq.false&order=created_at.desc&limit={limit}"
    );

    let thread_values = supabase_get(&http, &threads_url, &supabase_key).await?;
    let mut thread_titles = HashMap::new();
    let threads: Vec<AgentThreadRow> = thread_values
        .iter()
        .map(|value| {
            let id = json_string(value, "id").unwrap_or_default();
            let title = json_string(value, "title").unwrap_or_else(|| "Remote thread".to_string());
            thread_titles.insert(id.clone(), title.clone());
            AgentThreadRow {
                id,
                title,
                repo: json_string(value, "repo").unwrap_or_default(),
                base_branch: json_string(value, "base_branch").unwrap_or_default(),
                archived_at: json_string(value, "archived_at"),
                created_at: json_string(value, "created_at"),
                updated_at: json_string(value, "updated_at"),
                task_count: 0,
                active_task_count: 0,
                latest_task_at: None,
            }
        })
        .collect();

    let task_values = supabase_get(&http, &tasks_url, &supabase_key).await?;
    let tasks: Vec<AgentTaskRow> = task_values
        .iter()
        .map(|value| {
            let thread_id = json_string(value, "thread_id").unwrap_or_default();
            AgentTaskRow {
                id: json_string(value, "id").unwrap_or_default(),
                thread_id: thread_id.clone(),
                thread_title: thread_titles.get(&thread_id).cloned(),
                prompt: json_string(value, "prompt").unwrap_or_default(),
                status: json_string(value, "status").unwrap_or_else(|| "unknown".to_string()),
                branch: json_string(value, "branch"),
                pr_url: json_string(value, "pr_url"),
                pr_state: json_string(value, "pr_state"),
                exit_reason: json_string(value, "exit_reason"),
                error_message: json_string(value, "error_message"),
                started_at: json_string(value, "started_at"),
                finished_at: json_string(value, "finished_at"),
                created_at: json_string(value, "created_at"),
                updated_at: json_string(value, "updated_at"),
                last_event_seq: json_i32(value, "last_event_seq"),
                event_count: json_i64(value, "event_count"),
                latest_event_kind: None,
                latest_payload: None,
            }
        })
        .collect();

    Ok((threads, tasks))
}

pub(crate) async fn supabase_get(http: &reqwest::Client, url: &str, key: &str) -> Result<Vec<Value>, String> {
    let response = http
        .get(url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"))
        .header("apikey", key)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        tracing::error!(
            "agent tasks supabase http error: status={} body={}",
            status.as_u16(),
            body.chars().take(300).collect::<String>()
        );
        return Err(format!("supabase http {}", status.as_u16()));
    }
    serde_json::from_str::<Vec<Value>>(&body).map_err(|error| error.to_string())
}
