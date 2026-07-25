use std::{collections::HashMap, sync::Mutex};

use once_cell::sync::Lazy;

use crate::shared::{normalize_base_branch, normalize_repo_url, now_label, now_ms};
use crate::types::{
    AgentTaskRow, AgentThreadRow, AgentsDataConfig, AgentsSnapshot, AgentsSummary,
    DispatchTaskRequest, ThreadContextResponse,
};

pub(crate) static RUNTIME_STATE: Lazy<Mutex<RuntimeMemoryState>> =
    Lazy::new(|| Mutex::new(RuntimeMemoryState::default()));

#[derive(Default)]
pub(crate) struct RuntimeMemoryState {
    threads: HashMap<String, AgentThreadRow>,
    tasks: Vec<AgentTaskRow>,
}

pub(crate) fn remember_runtime_task(request: &DispatchTaskRequest, branch: Option<String>) {
    let now = now_label();
    if let Ok(mut state) = RUNTIME_STATE.lock() {
        let title = request
            .thread_title
            .clone()
            .unwrap_or_else(|| request.prompt.chars().take(80).collect::<String>());
        state.threads.insert(
            request.thread_id.clone(),
            AgentThreadRow {
                id: request.thread_id.clone(),
                title,
                repo: normalize_repo_url(&request.repo).unwrap_or_else(|_| request.repo.clone()),
                base_branch: normalize_base_branch(request.base_branch.as_deref())
                    .unwrap_or_else(|_| "dev".to_string()),
                archived_at: None,
                created_at: Some(now.clone()),
                updated_at: Some(now.clone()),
                task_count: 1,
                active_task_count: 1,
                latest_task_at: Some(now.clone()),
            },
        );
        state.tasks.insert(
            0,
            AgentTaskRow {
                id: request.task_id.clone(),
                thread_id: request.thread_id.clone(),
                thread_title: request.thread_title.clone(),
                prompt: request.prompt.clone(),
                status: "running".to_string(),
                branch,
                pr_url: None,
                pr_state: None,
                exit_reason: None,
                error_message: None,
                started_at: Some(now.clone()),
                finished_at: None,
                created_at: Some(now.clone()),
                updated_at: Some(now),
                last_event_seq: -1,
                event_count: 0,
                latest_event_kind: Some("dispatch".to_string()),
                latest_payload: None,
            },
        );
        if state.tasks.len() > 200 {
            state.tasks.truncate(200);
        }
    }
}

pub(crate) fn runtime_snapshot(
    limit: i64,
    config: AgentsDataConfig,
    mut errors: Vec<String>,
) -> AgentsSnapshot {
    let state = RUNTIME_STATE.lock().ok();
    let (threads, tasks) = if let Some(state) = state {
        let mut threads = state.threads.values().cloned().collect::<Vec<_>>();
        threads.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        let tasks = state
            .tasks
            .iter()
            .take(limit as usize)
            .cloned()
            .collect::<Vec<_>>();
        (threads, tasks)
    } else {
        errors.push("runtime memory state lock unavailable".to_string());
        (Vec::new(), Vec::new())
    };
    AgentsSnapshot {
        ok: true,
        source: "runtime-memory".to_string(),
        generated_at_ms: now_ms(),
        summary: summarize(&threads, &tasks),
        threads,
        tasks,
        errors,
        config,
    }
}

pub(crate) fn runtime_thread_context(
    thread_id: &str,
    limit: i64,
    mut errors: Vec<String>,
) -> ThreadContextResponse {
    let mut tasks = if let Ok(state) = RUNTIME_STATE.lock() {
        state
            .tasks
            .iter()
            .filter(|task| task.thread_id == thread_id)
            .take(limit as usize)
            .cloned()
            .collect::<Vec<_>>()
    } else {
        errors.push("runtime memory state lock unavailable".to_string());
        Vec::new()
    };
    tasks.reverse();
    ThreadContextResponse {
        ok: true,
        source: "runtime-memory".to_string(),
        thread_id: thread_id.to_string(),
        generated_at_ms: now_ms(),
        tasks,
        errors,
    }
}

pub(crate) fn summarize(threads: &[AgentThreadRow], tasks: &[AgentTaskRow]) -> AgentsSummary {
    let mut summary = AgentsSummary {
        thread_count: threads.len(),
        task_count: tasks.len(),
        ..AgentsSummary::default()
    };

    for task in tasks {
        match task.status.as_str() {
            "queued" | "running" | "streaming" => summary.running_count += 1,
            "failed" | "cancelled" => summary.failed_count += 1,
            "done" | "pushed" | "pr_open" | "pr_merged" | "pr_closed" => {
                summary.done_count += 1;
            }
            _ => {}
        }
        if task.pr_url.is_some() {
            summary.pr_count += 1;
        }
    }

    summary
}
