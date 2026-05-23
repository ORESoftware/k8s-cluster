//! Notification rules + recent dispatches for a tenant.

use axum::extract::{Path, State};
use maud::{Markup, html};
use uuid::Uuid;

use crate::notifications::{DispatchStatus, NotificationChannel};
use crate::state::AppState;

use super::errors;
use super::layout::{empty_row, section_header, short_id};
use super::time::rel;

pub async fn page_fragment(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
) -> Markup {
    render_panel(&state, tenant_id).await
}

pub(super) async fn render_panel(state: &AppState, tenant_id: Uuid) -> Markup {
    // Parallelize the two reads so the panel render time is bounded by
    // the slower of the two queries rather than their sum.
    let (rules_r, dispatches_r) = tokio::join!(
        state.notifications.list_rules(tenant_id),
        state.notifications.list_dispatches(tenant_id, 50),
    );

    let rules = match rules_r {
        Ok(r) => r,
        Err(e) => return errors::sanitized("list notification rules", &e),
    };
    let dispatches = match dispatches_r {
        Ok(d) => d,
        Err(e) => {
            // Don't blank out the rules table if dispatches happen to
            // fail; surface the error and continue with an empty list.
            tracing::warn!(error = %e, "admin: list dispatches failed");
            Vec::new()
        }
    };

    html! {
        (section_header(
            "Notification rules",
            Some("When to notify (kind + params), and how (channel + target). \
                  Rules are evaluated by the `notifications.evaluate_rules` job."),
        ))
        div class="table-wrap" {
            table {
                thead {
                    tr {
                        th { "Kind" }
                        th { "Name" }
                        th { "Channel" }
                        th { "Target" }
                        th class="num" { "Throttle / day" }
                        th { "Enabled" }
                    }
                }
                tbody {
                    @if rules.is_empty() {
                        (empty_row(6, "No notification rules configured for this tenant."))
                    }
                    @for r in &rules {
                        tr {
                            td { code class="short-id" { (r.kind) } }
                            td { (r.name) }
                            td { (channel_badge(r.channel)) }
                            td { code class="tight" { (truncate(&r.target, 64)) } }
                            td class="num" { (r.throttle_per_day) }
                            td {
                                @if r.enabled {
                                    span class="badge badge-ok" { "enabled" }
                                } @else {
                                    span class="badge badge-muted" { "disabled" }
                                }
                            }
                        }
                    }
                }
            }
        }

        (section_header(
            "Recent dispatches",
            Some("Last 50 outbound notification attempts."),
        ))
        div class="table-wrap" {
            table {
                thead {
                    tr {
                        th { "When" }
                        th { "Rule" }
                        th { "Channel" }
                        th { "Target" }
                        th { "Status" }
                        th { "Resource" }
                        th { "Error" }
                    }
                }
                tbody {
                    @if dispatches.is_empty() {
                        (empty_row(7, "No dispatches recorded yet."))
                    }
                    @for d in &dispatches {
                        tr {
                            td { (rel(d.created_at)) }
                            td { (short_id(d.rule_id)) }
                            td { (channel_badge(d.channel)) }
                            td class="tight" { code { (truncate(&d.target, 40)) } }
                            td { (dispatch_status_badge(d.status)) }
                            td class="tight muted" {
                                @match d.target_resource.as_deref() {
                                    Some(r) => (truncate(r, 40)),
                                    None => "—",
                                }
                            }
                            td class="tight muted" {
                                @match d.error.as_deref() {
                                    Some(e) => (truncate(e, 60)),
                                    None => "—",
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn channel_badge(c: NotificationChannel) -> Markup {
    let text = match c {
        NotificationChannel::Email => "email",
        NotificationChannel::Webhook => "webhook",
        NotificationChannel::Slack => "slack",
        NotificationChannel::Sms => "sms",
    };
    html! { span class="badge badge-muted" { (text) } }
}

fn dispatch_status_badge(s: DispatchStatus) -> Markup {
    let (class, text) = match s {
        DispatchStatus::Pending => ("badge badge-pending", "pending"),
        DispatchStatus::Sending => ("badge badge-pending", "sending"),
        DispatchStatus::Sent => ("badge badge-ok", "sent"),
        DispatchStatus::Failed => ("badge badge-fail", "failed"),
        DispatchStatus::Throttled => ("badge badge-muted", "throttled"),
        DispatchStatus::Suppressed => ("badge badge-muted", "suppressed"),
    };
    html! { span class=(class) { (text) } }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('…');
    out
}
