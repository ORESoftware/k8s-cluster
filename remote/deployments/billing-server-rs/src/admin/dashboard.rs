//! `/admin/` overview: at-a-glance stats + live status pill + recent runs.

use axum::extract::State;
use maud::{Markup, html};

use crate::providers::connection::ConnectionCounts;
use crate::scheduler::JobCounts;
use crate::state::AppState;

use super::layout::{
    self, NavSection, caption, empty_row, job_run_status_badge, section_header, short_id,
    stat_card,
};
use super::time::rel;

pub async fn page(State(state): State<AppState>) -> Markup {
    // Fan out the four reads in parallel: dashboard latency is dominated
    // by the slowest query rather than their sum. We use `tokio::join!`
    // (not `try_join!`) so a single transient failure degrades the
    // affected card to its zero/empty default rather than blanking the
    // whole page.
    let (tenant_r, conn_r, job_r, runs_r) = tokio::join!(
        state.tenants.count(),
        state.connections.counts(None),
        state.scheduler.counts(None),
        state.scheduler.recent_runs(None, 12),
    );

    let tenant_count = tenant_r.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "admin dashboard: tenant count failed");
        0
    });
    let conn_counts = conn_r.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "admin dashboard: connection counts failed");
        ConnectionCounts { total: 0, active: 0, failing: 0 }
    });
    let job_counts = job_r.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "admin dashboard: job counts failed");
        JobCounts { total: 0, enabled: 0, due_now: 0 }
    });
    let recent_runs = runs_r.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "admin dashboard: recent runs failed");
        Vec::new()
    });

    let conn_hint = if conn_counts.failing > 0 {
        format!("{} failing — investigate", conn_counts.failing)
    } else {
        "all healthy".to_string()
    };
    let jobs_hint = if job_counts.due_now > 0 {
        format!("{} due now", job_counts.due_now)
    } else {
        "nothing overdue".to_string()
    };

    let body = html! {
        h1 { "Dashboard" }
        (caption("Read-mostly admin for the billing server. The status pill auto-refreshes every 5s."))

        div class="grid grid-stats" style="margin-top: 16px;" {
            (stat_card("Tenants", format_count(tenant_count), "Total tenants across all regions."))
            (stat_card("Active connections", format!("{} / {}", conn_counts.active, conn_counts.total), conn_hint))
            (stat_card("Scheduled jobs", format!("{} / {}", job_counts.enabled, job_counts.total), jobs_hint))
            (stat_card("Server", env!("CARGO_PKG_VERSION"), "Build version"))
        }

        (section_header(
            "Recent job runs",
            Some("Most-recent 12 attempts across all tenants. Click a tenant id to drill in."),
        ))
        div class="table-wrap" {
            table {
                thead {
                    tr {
                        th { "Status" }
                        th { "Tenant" }
                        th { "Job" }
                        th class="num" { "Attempt" }
                        th { "Scheduled" }
                        th class="num" { "Duration" }
                        th { "Error" }
                    }
                }
                tbody {
                    @if recent_runs.is_empty() {
                        (empty_row(7, "No runs yet. The scheduler will populate this once jobs fire."))
                    }
                    @for r in &recent_runs {
                        tr {
                            td { (job_run_status_badge(r.status)) }
                            td {
                                @if let Some(t) = r.tenant_id {
                                    a href=(format!("/admin/tenants/{t}")) { (short_id(t)) }
                                } @else {
                                    span class="muted" { "system" }
                                }
                            }
                            td { (short_id(r.job_id)) }
                            td class="num" { (r.attempt) }
                            td { (rel(r.scheduled_for)) }
                            td class="num" {
                                @match r.duration_ms {
                                    Some(ms) => (format!("{ms} ms")),
                                    None => span class="muted" { "—" },
                                }
                            }
                            td class="tight muted" {
                                @match r.error.as_deref() {
                                    Some(e) => (truncate(e, 80)),
                                    None => "—",
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    layout::page("Dashboard", NavSection::Dashboard, body)
}

/// The little pill in the navbar — refreshed via `hx-trigger="every 5s"`.
pub async fn status_fragment(State(state): State<AppState>) -> Markup {
    let ok = sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await
        .is_ok();
    if ok {
        html! {
            span class="dot dot-ok" {}
            span { "ready" }
        }
    } else {
        html! {
            span class="dot dot-fail" {}
            span { "db down" }
        }
    }
}

fn format_count(n: i64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_count_inserts_commas() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1,000");
        assert_eq!(format_count(12_345), "12,345");
        assert_eq!(format_count(1_234_567), "1,234,567");
    }

    #[test]
    fn truncate_respects_char_bound() {
        assert_eq!(truncate("short", 80), "short");
        assert_eq!(truncate("éééééé", 3), "ééé…");
    }
}
