//! Scheduled jobs table + run-now / toggle HTMX actions.

use axum::extract::{Path, State};
use maud::{Markup, html};
use uuid::Uuid;

use crate::error::AppResult;
use crate::scheduler::{ScheduleKind, ScheduledJob};
use crate::state::AppState;

use super::layout::{empty_row, enabled_badge, flash_error, section_header, short_id};
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
        Err(e) => return flash_error(e.to_string()),
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
                    @for j in &rows { (job_row(j)) }
                }
            }
        }
    }
}

pub async fn run_now(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
) -> AppResult<Markup> {
    state.scheduler.run_now(job_id).await?;
    let job = state.scheduler.get(job_id).await?;
    Ok(job_row(&job))
}

pub async fn toggle(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
) -> AppResult<Markup> {
    let job = state.scheduler.get(job_id).await?;
    state.scheduler.set_enabled(job_id, !job.enabled).await?;
    let job = state.scheduler.get(job_id).await?;
    Ok(job_row(&job))
}

fn job_row(j: &ScheduledJob) -> Markup {
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
                    hx-post=(format!("/admin/jobs/{}/run-now", j.id))
                    hx-target=(format!("#job-{}", j.id))
                    hx-swap="outerHTML"
                {
                    button type="submit" class="btn btn-primary" { "Run now" }
                }
                form
                    class="inline"
                    hx-post=(format!("/admin/jobs/{}/toggle", j.id))
                    hx-target=(format!("#job-{}", j.id))
                    hx-swap="outerHTML"
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
