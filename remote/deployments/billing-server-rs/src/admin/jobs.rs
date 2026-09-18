//! Scheduled jobs table + run-now / toggle HTMX actions.

use axum::extract::{Path, State};
use maud::{Markup, html};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::scheduler::{ScheduleKind, ScheduledJob};
use crate::state::AppState;

use super::errors;
use super::layout::{empty_row, enabled_badge, section_header, short_id};
use super::time::rel;

pub async fn table_fragment(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
) -> Markup {
    render_table(&state, tenant_id).await
}

pub(super) async fn render_table(state: &AppState, tenant_id: Uuid) -> Markup {
    let rows = match state.scheduler.list(Some(tenant_id)).await {
        Ok(r) => r,
        Err(e) => return errors::sanitized("list scheduled jobs", &e),
    };

    html! {
        (section_header(
            "Scheduled jobs",
            Some("Durable cron / interval / one-shot jobs. Run-now forces next_run_at = now()."),
        ))
        div class="table-wrap" {
            table {
                thead {
                    tr {
                        th { "Kind" }
                        th { "Name" }
                        th { "Schedule" }
                        th { "Next run" }
                        th { "Last run" }
                        th { "Enabled" }
                        th class="num" { "Actions" }
                    }
                }
                tbody {
                    @if rows.is_empty() {
                        (empty_row(7, "No scheduled jobs for this tenant yet."))
                    }
                    @for j in &rows { (job_row(tenant_id, j)) }
                }
            }
        }
    }
}

/// `POST /admin/tenants/{tid}/jobs/{job_id}/run-now`.
///
/// The tenant id is part of the path so the URL can't be reused across
/// tenants (defense in depth) and so we can verify ownership before any
/// side effect. A mismatch returns 404 — same response as a non-existent
/// id — so an attacker can't probe which job ids belong to which tenant.
pub async fn run_now(
    State(state): State<AppState>,
    Path((tenant_id, job_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Markup> {
    let job = state.scheduler.get(job_id).await?;
    if job.tenant_id != Some(tenant_id) {
        return Err(AppError::NotFound(format!("job {job_id} not found in tenant")));
    }
    state.scheduler.run_now(job_id).await?;
    let job = state.scheduler.get(job_id).await?;
    tracing::info!(
        admin.action = "job.run_now",
        admin.tenant_id = %tenant_id,
        admin.job_id = %job_id,
        admin.job_name = %job.name,
        "admin: run-now requested"
    );
    Ok(job_row(tenant_id, &job))
}

/// `POST /admin/tenants/{tid}/jobs/{job_id}/toggle`. Same tenant-ownership
/// check as `run_now`.
pub async fn toggle(
    State(state): State<AppState>,
    Path((tenant_id, job_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Markup> {
    let job = state.scheduler.get(job_id).await?;
    if job.tenant_id != Some(tenant_id) {
        return Err(AppError::NotFound(format!("job {job_id} not found in tenant")));
    }
    let new_enabled = !job.enabled;
    state.scheduler.set_enabled(job_id, new_enabled).await?;
    let job = state.scheduler.get(job_id).await?;
    tracing::info!(
        admin.action = "job.toggle",
        admin.tenant_id = %tenant_id,
        admin.job_id = %job_id,
        admin.job_name = %job.name,
        admin.job_enabled = new_enabled,
        "admin: job enabled-state toggled"
    );
    Ok(job_row(tenant_id, &job))
}

fn job_row(tenant_id: Uuid, j: &ScheduledJob) -> Markup {
    html! {
        tr id=(format!("job-{}", j.id)) {
            td {
                code class="short-id" { (j.kind) }
                div class="tight muted" { (short_id(j.id)) }
            }
            td { (j.name) }
            td { (schedule_summary(j)) }
            td { (rel(j.next_run_at)) }
            td {
                @match j.last_run_at {
                    Some(t) => (rel(t)),
                    None => span class="muted" { "—" },
                }
            }
            td { (enabled_badge(j.enabled)) }
            td class="num row-actions" {
                form
                    class="inline"
                    hx-post=(format!("/admin/tenants/{}/jobs/{}/run-now", tenant_id, j.id))
                    hx-target=(format!("#job-{}", j.id))
                    hx-swap="outerHTML"
                    hx-confirm=(format!("Run {} now?", j.name))
                {
                    button type="submit" class="btn btn-primary" { "Run now" }
                }
                form
                    class="inline"
                    hx-post=(format!("/admin/tenants/{}/jobs/{}/toggle", tenant_id, j.id))
                    hx-target=(format!("#job-{}", j.id))
                    hx-swap="outerHTML"
                    hx-confirm=(format!(
                        "{} {}?",
                        if j.enabled { "Disable" } else { "Enable" },
                        j.name,
                    ))
                {
                    button type="submit" class="btn" {
                        @if j.enabled { "Disable" } @else { "Enable" }
                    }
                }
            }
        }
    }
}

fn schedule_summary(j: &ScheduledJob) -> Markup {
    match j.schedule_kind {
        ScheduleKind::Cron => html! {
            span class="badge badge-muted" { "cron" }
            code class="tight" style="margin-left: 6px;" {
                (j.cron_expr.as_deref().unwrap_or("?"))
            }
            div class="tight muted" { (j.timezone) }
        },
        ScheduleKind::Interval => html! {
            span class="badge badge-muted" { "interval" }
            span class="tight" style="margin-left: 6px;" {
                (format_interval(j.interval_seconds.unwrap_or(0)))
            }
        },
        ScheduleKind::OneShot => html! {
            span class="badge badge-muted" { "one-shot" }
        },
    }
}

fn format_interval(seconds: i32) -> String {
    let s = seconds as i64;
    if s <= 0 {
        return "—".into();
    }
    if s % 86_400 == 0 {
        return format!("every {}d", s / 86_400);
    }
    if s % 3_600 == 0 {
        return format!("every {}h", s / 3_600);
    }
    if s % 60 == 0 {
        return format!("every {}m", s / 60);
    }
    format!("every {s}s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_collapse_to_nice_units() {
        assert_eq!(format_interval(0), "—");
        assert_eq!(format_interval(30), "every 30s");
        assert_eq!(format_interval(60), "every 1m");
        assert_eq!(format_interval(300), "every 5m");
        assert_eq!(format_interval(3_600), "every 1h");
        assert_eq!(format_interval(18_000), "every 5h");
        assert_eq!(format_interval(86_400), "every 1d");
        assert_eq!(format_interval(2 * 86_400), "every 2d");
    }
}
