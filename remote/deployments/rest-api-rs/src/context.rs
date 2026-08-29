use std::collections::{HashMap, HashSet};

use axum::{
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::db::{
    connect_postgres, fetch_agent_breadcrumb_tail_from_postgres,
    fetch_thread_context_from_postgres, upsert_known_git_repo_to_postgres,
};
use crate::metrics::record_request;
use crate::shared::{
    context_candidate_limit, first_env, normalize_base_branch, normalize_context_mode,
    normalize_context_project_id, normalize_repo_url, now_ms, postgres_database_url,
    public_data_source_error, row_opt_string, row_string,
};
use crate::types::{
    AgentBreadcrumbRow, AgentContextCandidate, AgentContextCandidatesRequest,
    AgentContextCandidatesResponse, AgentTaskRow, DispatchTaskRequest, ThreadRepoConfig,
    BREADCRUMB_CANDIDATE_PREFIX, CONTEXT_KIND_BLOB, CONTEXT_KIND_BREADCRUMB, CONTEXT_KIND_TASK,
};

pub(crate) fn agent_context_embedding_model() -> String {
    first_env(&["AGENT_CONTEXT_EMBEDDING_MODEL", "OPENAI_EMBEDDING_MODEL"])
        .unwrap_or_else(|| "text-embedding-3-small".to_string())
}

pub(crate) fn configured_secret_list(keys: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for key in keys {
        let Some(raw) = first_env(&[*key]) else {
            continue;
        };
        if raw.trim_start().starts_with('[') {
            if let Ok(values) = serde_json::from_str::<Vec<String>>(&raw) {
                out.extend(
                    values
                        .into_iter()
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty()),
                );
                continue;
            }
        }
        out.extend(
            raw.split([',', '\n'])
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
        );
    }
    out
}

pub(crate) async fn embed_context_query(prompt: &str) -> Result<(String, Vec<f64>), String> {
    let api_keys = configured_secret_list(&["OPENAI_API_KEYS_JSON", "OPENAI_API_KEY"]);
    if api_keys.is_empty() {
        return Err("no OpenAI key configured for context embeddings".to_string());
    }
    let model = agent_context_embedding_model();
    let base_url = first_env(&["OPENAI_BASE_URL"])
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string())
        .trim_end_matches('/')
        .to_string();
    let client = reqwest::Client::new();
    let total_keys = api_keys.len();
    let mut last_error = String::new();
    for (index, api_key) in api_keys.into_iter().enumerate() {
        let response = client
            .post(format!("{base_url}/embeddings"))
            .bearer_auth(api_key)
            .json(&json!({
                "model": model,
                "input": prompt,
            }))
            .send()
            .await;
        let response = match response {
            Ok(value) => value,
            Err(error) => {
                last_error = format!("key {}/{} transport error: {error}", index + 1, total_keys);
                continue;
            }
        };
        let status = response.status();
        let body = response.text().await.map_err(|error| error.to_string())?;
        if !status.is_success() {
            last_error = format!(
                "key {}/{} failed with HTTP {}",
                index + 1,
                total_keys,
                status.as_u16()
            );
            continue;
        }
        let value = serde_json::from_str::<Value>(&body).map_err(|error| error.to_string())?;
        let Some(embedding) = value
            .get("data")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("embedding"))
            .and_then(json_embedding_to_vec)
            .filter(|values| !values.is_empty())
        else {
            last_error = format!(
                "key {}/{} returned no numeric embedding vector",
                index + 1,
                total_keys
            );
            continue;
        };
        return Ok((model, embedding));
    }
    Err(format!(
        "all {total_keys} OpenAI embedding key(s) failed; last error: {last_error}"
    ))
}

pub(crate) fn json_embedding_to_vec(value: &Value) -> Option<Vec<f64>> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_f64).collect::<Vec<f64>>())
}

pub(crate) fn cosine_similarity(a: &[f64], b: &[f64]) -> Option<f64> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let mut dot = 0.0;
    let mut a_norm = 0.0;
    let mut b_norm = 0.0;
    for (left, right) in a.iter().zip(b.iter()) {
        dot += left * right;
        a_norm += left * left;
        b_norm += right * right;
    }
    if a_norm == 0.0 || b_norm == 0.0 {
        return None;
    }
    Some(dot / (a_norm.sqrt() * b_norm.sqrt()))
}

pub(crate) fn context_tokens(value: &str) -> HashSet<String> {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(str::to_lowercase)
        .filter(|item| item.len() >= 3)
        .collect()
}

pub(crate) fn lexical_context_score(prompt: &str, title: &str, blob: &str) -> f64 {
    let query = context_tokens(prompt);
    if query.is_empty() {
        return 0.0;
    }
    let title_tokens = context_tokens(title);
    let blob_tokens = context_tokens(blob);
    let title_hits = query.intersection(&title_tokens).count() as f64;
    let blob_hits = query.intersection(&blob_tokens).count() as f64;
    ((title_hits * 3.0) + blob_hits) / query.len() as f64
}

pub(crate) async fn ensure_agent_context_schema(client: &tokio_postgres::Client) -> Result<(), String> {
    client
        .batch_execute(
            r#"
            create table if not exists agent_context_blobs (
              id uuid primary key default gen_random_uuid(),
              project_id varchar(120) default 'default' not null,
              repo_id uuid references known_git_repos(id),
              context_id varchar(200) not null,
              context_title varchar(300) not null,
              context_blob text not null,
              status varchar(32) default 'active' not null,
              labels jsonb default '[]'::jsonb not null,
              meta_data jsonb default '{}'::jsonb not null,
              is_soft_deleted boolean default false not null,
              created_at timestamptz default now() not null,
              updated_at timestamptz default now() not null,
              created_by uuid,
              updated_by uuid
            );
            create unique index if not exists agent_context_blobs_project_repo_context_active_uq
              on agent_context_blobs (project_id, repo_id, context_id)
              where is_soft_deleted = false;
            create index if not exists agent_context_blobs_repo_id_idx
              on agent_context_blobs (repo_id)
              where is_soft_deleted = false;
            create index if not exists agent_context_blobs_project_id_idx
              on agent_context_blobs (project_id)
              where is_soft_deleted = false;
            create index if not exists agent_context_blobs_updated_at_idx
              on agent_context_blobs (updated_at desc)
              where is_soft_deleted = false;

            create table if not exists agent_context_embeddings (
              id uuid primary key default gen_random_uuid(),
              context_blob_id uuid not null references agent_context_blobs(id),
              embedding_model varchar(120) not null,
              embedding jsonb not null,
              embedding_dimensions integer not null,
              content_sha256 varchar(64) not null,
              created_at timestamptz default now() not null
            );
            create unique index if not exists agent_context_embeddings_blob_model_sha_uq
              on agent_context_embeddings (context_blob_id, embedding_model, content_sha256);
            create index if not exists agent_context_embeddings_blob_id_idx
              on agent_context_embeddings (context_blob_id);
            "#,
        )
        .await
        .map_err(|error| error.to_string())
}

pub(crate) fn task_context_id(task_id: &str) -> String {
    format!("task:{task_id}")
}

pub(crate) fn task_id_from_context_id(context_id: &str) -> Option<&str> {
    context_id
        .strip_prefix("task:")
        .filter(|value| !value.is_empty())
}

pub(crate) fn truncate_for_context_blob(value: &str, limit: usize) -> String {
    let trimmed = value.trim();
    let mut chars = trimmed.chars();
    let truncated = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_none() {
        trimmed.to_string()
    } else {
        format!("{truncated}...")
    }
}

pub(crate) fn format_task_context_blob(task: &AgentTaskRow) -> String {
    let mut lines = vec![
        format!("taskId: {}", task.id),
        format!("threadId: {}", task.thread_id),
        format!("status: {}", task.status),
    ];
    if let Some(branch) = task.branch.as_deref().filter(|value| !value.is_empty()) {
        lines.push(format!("branch: {branch}"));
    }
    if let Some(exit_reason) = task
        .exit_reason
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("exit: {exit_reason}"));
    }
    if let Some(error_message) = task
        .error_message
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        lines.push(format!(
            "error: {}",
            truncate_for_context_blob(error_message, 1200)
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "prompt: {}",
        truncate_for_context_blob(&task.prompt, 4000)
    ));
    if let Some(latest) = task
        .latest_payload
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        lines.push(String::new());
        lines.push(format!(
            "latestEvent: {}",
            truncate_for_context_blob(latest, 4000)
        ));
    }
    lines.join("\n")
}

pub(crate) fn task_context_candidate(task: &AgentTaskRow, prompt: &str) -> AgentContextCandidate {
    let title = format!(
        "Previous task {}",
        task.id.chars().take(8).collect::<String>()
    );
    let blob = format_task_context_blob(task);
    AgentContextCandidate {
        context_id: task_context_id(&task.id),
        project_id: "thread".to_string(),
        repo_id: None,
        context_title: title,
        context_blob: blob.clone(),
        score: lexical_context_score(prompt, &task.prompt, &blob),
        match_source: CONTEXT_KIND_TASK.to_string(),
        embedding_model: None,
        updated_at: task.updated_at.clone().or_else(|| task.created_at.clone()),
        kind: CONTEXT_KIND_TASK.to_string(),
    }
}

pub(crate) async fn fetch_thread_task_context_candidates_from_postgres(
    thread_id: &str,
    prompt: &str,
    limit: i64,
) -> Result<Vec<AgentContextCandidate>, String> {
    Ok(fetch_thread_context_from_postgres(thread_id, limit)
        .await?
        .iter()
        .map(|task| task_context_candidate(task, prompt))
        .collect())
}

pub(crate) async fn fetch_blob_context_candidates_from_postgres(
    selected_ids: &[String],
    project_id: &str,
    repo_id: &str,
) -> Result<Vec<AgentContextCandidate>, String> {
    if selected_ids.is_empty() {
        return Ok(Vec::new());
    }
    let client = connect_postgres().await?;
    ensure_agent_context_schema(&client).await?;
    let rows = client
        .query(
            r#"
            select
              c.context_id,
              c.project_id,
              c.repo_id::text as repo_id,
              c.context_title,
              left(c.context_blob, 20000) as context_blob,
              to_char(c.updated_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as updated_at,
              e.embedding_model
            from agent_context_blobs c
            left join lateral (
              select embedding_model
              from agent_context_embeddings
              where context_blob_id = c.id
              order by created_at desc
              limit 1
            ) e on true
            where c.is_soft_deleted = false
              and c.status = 'active'
              and c.project_id = $1
              and c.repo_id = $2::text::uuid
              and c.context_id = any($3)
            "#,
            &[&project_id, &repo_id, &selected_ids],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(rows
        .iter()
        .map(|row| AgentContextCandidate {
            context_id: row_string(row, "context_id"),
            project_id: row_string(row, "project_id"),
            repo_id: row_opt_string(row, "repo_id"),
            context_title: row_string(row, "context_title"),
            context_blob: row_string(row, "context_blob"),
            score: 1.0,
            match_source: "selected".to_string(),
            embedding_model: row_opt_string(row, "embedding_model"),
            updated_at: row_opt_string(row, "updated_at"),
            kind: CONTEXT_KIND_BLOB.to_string(),
        })
        .collect())
}

pub(crate) async fn fetch_breadcrumb_context_candidates_by_ids_from_postgres(
    thread_id: &str,
    selected_ids: &[String],
    repo_id: String,
) -> Result<Vec<AgentContextCandidate>, String> {
    let numeric_ids = selected_ids
        .iter()
        .filter_map(|id| {
            id.strip_prefix(BREADCRUMB_CANDIDATE_PREFIX)
                .and_then(|tail| tail.parse::<i64>().ok())
        })
        .collect::<Vec<_>>();
    if numeric_ids.is_empty() {
        return Ok(Vec::new());
    }
    let client = connect_postgres().await?;
    let rows = client
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
              and id = any($2)
            "#,
            &[&thread_id, &numeric_ids],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(rows
        .iter()
        .map(|row| {
            let breadcrumb = AgentBreadcrumbRow {
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
            };
            breadcrumb_row_to_candidate(breadcrumb, repo_id.clone())
        })
        .collect())
}

pub(crate) async fn fetch_selected_context_candidates_from_postgres(
    thread_id: &str,
    prompt: &str,
    selected_ids: &[String],
    repo_config: &ThreadRepoConfig,
) -> Result<Vec<AgentContextCandidate>, String> {
    let project_id = normalize_context_project_id(None)?;
    let mut breadcrumb_ids = Vec::new();
    let mut blob_ids = Vec::new();
    let mut task_ids = HashSet::new();
    for id in selected_ids {
        if id.starts_with(BREADCRUMB_CANDIDATE_PREFIX) {
            breadcrumb_ids.push(id.clone());
        } else if let Some(task_id) = task_id_from_context_id(id) {
            task_ids.insert(task_id.to_string());
        } else {
            blob_ids.push(id.clone());
        }
    }
    let repo = if blob_ids.is_empty() && breadcrumb_ids.is_empty() {
        None
    } else {
        Some(
            upsert_known_git_repo_to_postgres(
                &repo_config.repo,
                None,
                None,
                Some(&repo_config.base_branch),
            )
            .await?,
        )
    };
    let blob_candidates = if let Some(repo) = repo.as_ref() {
        fetch_blob_context_candidates_from_postgres(&blob_ids, &project_id, &repo.id).await?
    } else {
        Vec::new()
    };
    let breadcrumb_candidates = if let Some(repo) = repo.as_ref() {
        fetch_breadcrumb_context_candidates_by_ids_from_postgres(
            thread_id,
            &breadcrumb_ids,
            repo.id.clone(),
        )
        .await?
    } else {
        Vec::new()
    };
    let task_candidates = if task_ids.is_empty() {
        Vec::new()
    } else {
        fetch_thread_task_context_candidates_from_postgres(thread_id, prompt, 100)
            .await?
            .into_iter()
            .filter(|candidate| {
                task_id_from_context_id(&candidate.context_id)
                    .is_some_and(|task_id| task_ids.contains(task_id))
            })
            .collect::<Vec<_>>()
    };
    let mut by_id = blob_candidates
        .into_iter()
        .chain(breadcrumb_candidates)
        .chain(task_candidates)
        .map(|candidate| (candidate.context_id.clone(), candidate))
        .collect::<HashMap<_, _>>();
    Ok(selected_ids
        .iter()
        .filter_map(|id| by_id.remove(id))
        .collect::<Vec<_>>())
}

pub(crate) async fn fetch_agent_context_candidates_from_postgres(
    thread_id: &str,
    request: &AgentContextCandidatesRequest,
) -> Result<AgentContextCandidatesResponse, String> {
    let repo_url = normalize_repo_url(&request.repo)?;
    let base_branch = normalize_base_branch(request.base_branch.as_deref())?;
    let project_id = normalize_context_project_id(request.project_id.as_deref())?;
    let limit = context_candidate_limit(request.limit);
    let selected_ids = request.context_ids.clone().unwrap_or_default();
    if !selected_ids.is_empty() {
        let repo_config = ThreadRepoConfig {
            repo: repo_url,
            base_branch,
            thread_title: None,
        };
        // Use the helper-based path here (not the dispatch path) to avoid an
        // async recursion cycle through fetch_selected_agent_context_*.
        let candidates = fetch_selected_context_candidates_from_postgres(
            thread_id,
            &request.prompt,
            &selected_ids,
            &repo_config,
        )
        .await?;
        return Ok(AgentContextCandidatesResponse {
            ok: true,
            source: "postgres".to_string(),
            thread_id: thread_id.to_string(),
            generated_at_ms: now_ms(),
            project_id,
            repo_id: None,
            candidates,
            errors: Vec::new(),
        });
    }
    let repo = upsert_known_git_repo_to_postgres(&repo_url, None, None, Some(&base_branch)).await?;
    let client = connect_postgres().await?;
    ensure_agent_context_schema(&client).await?;

    let rows = client
        .query(
            r#"
            select
              c.context_id,
              c.project_id,
              c.repo_id::text as repo_id,
              c.context_title,
              left(c.context_blob, 20000) as context_blob,
              to_char(c.updated_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as updated_at,
              e.embedding_model,
              e.embedding
            from agent_context_blobs c
            left join lateral (
              select embedding_model, embedding
              from agent_context_embeddings
              where context_blob_id = c.id
              order by created_at desc
              limit 1
            ) e on true
            where c.is_soft_deleted = false
              and c.status = 'active'
              and c.project_id = $1
              and c.repo_id = $2::text::uuid
            order by c.updated_at desc
            limit 200
            "#,
            &[&project_id, &repo.id],
        )
        .await
        .map_err(|error| error.to_string())?;

    let mut errors = Vec::new();
    let query_embedding = match embed_context_query(&request.prompt).await {
        Ok(value) => Some(value),
        Err(error) => {
            errors.push(format!(
                "embedding ranking unavailable; using lexical fallback: {error}"
            ));
            None
        }
    };

    let mut candidates = rows
        .iter()
        .map(|row| {
            let title = row_string(row, "context_title");
            let blob = row_string(row, "context_blob");
            let embedding_model = row_opt_string(row, "embedding_model");
            let embedding_value = row.try_get::<_, Value>("embedding").ok();
            let embedding_score =
                query_embedding
                    .as_ref()
                    .and_then(|(query_model, query_vector)| {
                        let row_model = embedding_model.as_deref()?;
                        if row_model != query_model {
                            return None;
                        }
                        let row_vector =
                            embedding_value.as_ref().and_then(json_embedding_to_vec)?;
                        cosine_similarity(query_vector, &row_vector)
                    });
            let lexical_score = lexical_context_score(&request.prompt, &title, &blob);
            AgentContextCandidate {
                context_id: row_string(row, "context_id"),
                project_id: row_string(row, "project_id"),
                repo_id: row_opt_string(row, "repo_id"),
                context_title: title,
                context_blob: blob,
                score: embedding_score.unwrap_or(lexical_score),
                match_source: if embedding_score.is_some() {
                    "embedding".to_string()
                } else {
                    "lexical".to_string()
                },
                embedding_model,
                updated_at: row_opt_string(row, "updated_at"),
                kind: CONTEXT_KIND_BLOB.to_string(),
            }
        })
        .collect::<Vec<_>>();
    let task_candidates =
        match fetch_thread_task_context_candidates_from_postgres(thread_id, &request.prompt, 20)
            .await
        {
            Ok(items) => items,
            Err(error) => {
                errors.push(format!(
                    "thread task context unavailable; continuing without it: {error}"
                ));
                Vec::new()
            }
        };
    candidates.extend(task_candidates);
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.context_title.cmp(&b.context_title))
    });
    candidates.truncate(limit as usize);

    // Surface recent breadcrumbs alongside long-lived context blobs so the same
    // picker can include / exclude them with checkboxes. Breadcrumb candidates
    // ride the same `contextIds` rail using the `breadcrumb:<numeric-id>`
    // prefix, and the same `contextBlobs` payload using `kind: "breadcrumb"`.
    let breadcrumb_candidates =
        match fetch_breadcrumb_candidates_for_thread(thread_id, repo.id.clone()).await {
            Ok(items) => items,
            Err(error) => {
                errors.push(format!(
                    "breadcrumb candidates unavailable; continuing without them: {error}"
                ));
                Vec::new()
            }
        };
    candidates.extend(breadcrumb_candidates);

    Ok(AgentContextCandidatesResponse {
        ok: true,
        source: "postgres".to_string(),
        thread_id: thread_id.to_string(),
        generated_at_ms: now_ms(),
        project_id,
        repo_id: Some(repo.id),
        candidates,
        errors,
    })
}

/// How many recent breadcrumbs to surface in the context picker. The picker UI
/// is checkbox-based so this is a soft cap on visible rows, not a prompt
/// budget; the actual prompt cost is governed by which boxes the user keeps
/// checked.
pub(crate) const BREADCRUMB_CANDIDATE_LIMIT: i64 = 25;

pub(crate) async fn fetch_breadcrumb_candidates_for_thread(
    thread_id: &str,
    repo_id: String,
) -> Result<Vec<AgentContextCandidate>, String> {
    let rows =
        fetch_agent_breadcrumb_tail_from_postgres(thread_id, BREADCRUMB_CANDIDATE_LIMIT, None)
            .await?;
    let candidates = rows
        .into_iter()
        .map(|row| breadcrumb_row_to_candidate(row, repo_id.clone()))
        .collect();
    Ok(candidates)
}

pub(crate) fn breadcrumb_row_to_candidate(row: AgentBreadcrumbRow, repo_id: String) -> AgentContextCandidate {
    let summary = breadcrumb_payload_summary(&row.payload);
    let title = if summary.is_empty() {
        format!("breadcrumb · {} · {}", row.kind, row.emitted_at)
    } else {
        format!("breadcrumb · {} · {} · {summary}", row.kind, row.emitted_at)
    };
    let blob_payload = json!({
        "id": row.id,
        "kind": row.kind,
        "emittedAt": row.emitted_at,
        "taskId": row.task_id,
        "podName": row.pod_name,
        "branch": row.branch,
        "provider": row.provider,
        "payload": row.payload,
    });
    let blob = serde_json::to_string(&blob_payload)
        .unwrap_or_else(|_| format!("{{\"kind\":\"{}\"}}", row.kind));
    AgentContextCandidate {
        context_id: format!("{BREADCRUMB_CANDIDATE_PREFIX}{}", row.id),
        project_id: String::new(),
        repo_id: Some(repo_id),
        context_title: title,
        context_blob: blob,
        // Breadcrumbs sort below high-confidence semantic context but above
        // unrelated lexical noise. Operators decide via checkboxes; the score
        // only seeds default ordering.
        score: 0.55,
        match_source: CONTEXT_KIND_BREADCRUMB.to_string(),
        embedding_model: None,
        updated_at: Some(row.emitted_at),
        kind: CONTEXT_KIND_BREADCRUMB.to_string(),
    }
}

pub(crate) fn breadcrumb_payload_summary(payload: &Value) -> String {
    if let Some(object) = payload.as_object() {
        for key in [
            "summary",
            "message",
            "status",
            "branch",
            "kind",
            "exitReason",
        ] {
            if let Some(value) = object.get(key).and_then(Value::as_str) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    let mut snippet: String = trimmed.chars().take(80).collect();
                    if trimmed.chars().count() > 80 {
                        snippet.push('\u{2026}');
                    }
                    return snippet;
                }
            }
        }
    }
    String::new()
}

pub(crate) async fn fetch_selected_agent_context_from_postgres(
    request: &DispatchTaskRequest,
    repo_config: &ThreadRepoConfig,
) -> Result<Vec<AgentContextCandidate>, String> {
    let selected_ids = request.context_ids.clone().unwrap_or_default();
    let mode = normalize_context_mode(request.context_mode.as_deref(), selected_ids.len());
    if mode == "none" {
        return Ok(Vec::new());
    }
    if mode == "auto" && selected_ids.is_empty() {
        return Ok(fetch_agent_context_candidates_from_postgres(
            &request.thread_id,
            &AgentContextCandidatesRequest {
                prompt: request.prompt.clone(),
                repo: repo_config.repo.clone(),
                base_branch: Some(repo_config.base_branch.clone()),
                project_id: None,
                limit: Some(10),
                context_ids: None,
            },
        )
        .await?
        .candidates);
    }
    if selected_ids.is_empty() {
        return Ok(Vec::new());
    }

    fetch_selected_context_candidates_from_postgres(
        &request.thread_id,
        &request.prompt,
        &selected_ids,
        repo_config,
    )
    .await
}

pub(crate) async fn thread_context_candidates(
    Path(thread_id): Path<String>,
    Json(request): Json<AgentContextCandidatesRequest>,
) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/context-candidates",
        StatusCode::OK,
    );
    if request.prompt.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "prompt is required" })),
        )
            .into_response();
    }
    if postgres_database_url().is_none() {
        return Json(AgentContextCandidatesResponse {
            ok: true,
            source: "postgres".to_string(),
            thread_id,
            generated_at_ms: now_ms(),
            project_id: normalize_context_project_id(request.project_id.as_deref())
                .unwrap_or_else(|_| "default".to_string()),
            repo_id: None,
            candidates: Vec::new(),
            errors: vec![
                "postgres database URL is not configured; start with zero context or dispatch without selected context".to_string(),
            ],
        })
        .into_response();
    }
    match fetch_agent_context_candidates_from_postgres(&thread_id, &request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            tracing::error!("agent context candidate lookup failed: {error}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": public_data_source_error("postgres context candidates")
                })),
            )
                .into_response()
        }
    }
}
