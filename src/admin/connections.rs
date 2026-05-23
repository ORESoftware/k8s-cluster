//! Provider connections table + per-connection HTMX actions.

use axum::extract::{Path, State};
use maud::{Markup, html};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::providers::connection::ProviderConnection;
use crate::state::AppState;

use super::errors;
use super::layout::{connection_status_badge, empty_row, section_header, short_id};
use super::time::rel_opt;

/// Full table fragment (used for both initial tab paint and HTMX tab swap).
pub async fn table_fragment(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
) -> Markup {
    render_table(&state, tenant_id).await
}

pub(super) async fn render_table(state: &AppState, tenant_id: Uuid) -> Markup {
    let rows = match state.connections.list_for_tenant(tenant_id).await {
        Ok(r) => r,
        Err(e) => return errors::sanitized("list connections", &e),
    };

    html! {
        (section_header(
            "Provider connections",
            Some("OAuth / API-key / wallet handles registered for this tenant. \
                  Sync now enqueues an on-demand poll."),
        ))
        div class="table-wrap" {
            table {
                thead {
                    tr {
                        th { "Provider" }
                        th { "Label" }
                        th { "External id" }
                        th { "Status" }
                        th { "Last sync" }
                        th { "Last error" }
                        th class="num" { "Actions" }
                    }
                }
                tbody {
                    @if rows.is_empty() {
                        (empty_row(7, "No connections yet. Use the JSON API or OAuth start route to register one."))
                    }
                    @for c in &rows { (connection_row(tenant_id, c)) }
                }
            }
        }
    }
}

/// `POST /admin/tenants/{tid}/connections/{conn_id}/sync`.
///
/// Tenant-scoped path so the URL itself carries the ownership claim and
/// the audit log is unambiguous. We verify the connection actually
/// belongs to the path tenant via `connections.get(tenant_id, conn_id)`
/// (returns NotFound otherwise — same as a non-existent id, so this
/// can't be used to enumerate connection ids across tenants).
pub async fn sync_now(
    State(state): State<AppState>,
    Path((tenant_id, conn_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Markup> {
    let tenant = state.tenants.by_id(tenant_id).await?;
    let conn = state.connections.get(tenant.id, conn_id).await?;
    let region = tenant.region().map_err(|e| match e {
        AppError::BadRequest(m) => AppError::BadRequest(m),
        other => other,
    })?;

    let _ = state
        .scheduler
        .enqueue_one_shot(
            tenant.id,
            region,
            "sync.connection",
            format!("on-demand-conn-{}", conn_id),
            serde_json::json!({
                "connection_id": conn_id,
                "trigger": "admin_ui",
            }),
        )
        .await?;

    tracing::info!(
        admin.action = "connection.sync_now",
        admin.tenant_id = %tenant.id,
        admin.connection_id = %conn_id,
        admin.provider = %conn.provider.tag(),
        "admin: on-demand connection sync enqueued"
    );

    let conn = state.connections.get(tenant.id, conn_id).await?;
    Ok(connection_row(tenant_id, &conn))
}

fn connection_row(tenant_id: Uuid, c: &ProviderConnection) -> Markup {
    html! {
        tr id=(format!("conn-{}", c.id)) {
            td {
                span class="nowrap" { (c.provider.tag()) }
                div class="tight muted" { (auth_kind_label(c.auth_kind)) }
            }
            td {
                (c.display_label)
                div class="tight muted" { (short_id(c.id)) }
            }
            td {
                @match &c.external_account_id {
                    Some(s) => code class="short-id" { (s) },
                    None => span class="muted" { "—" },
                }
            }
            td { (connection_status_badge(c.status)) }
            td { (rel_opt(c.last_sync_at)) }
            td class="tight muted" {
                @match c.last_error.as_deref() {
                    Some(e) => (truncate(e, 80)),
                    None => "—",
                }
            }
            td class="num row-actions" {
                form
                    class="inline"
                    hx-post=(format!("/admin/tenants/{}/connections/{}/sync", tenant_id, c.id))
                    hx-target=(format!("#conn-{}", c.id))
                    hx-swap="outerHTML"
                    hx-confirm=(format!("Trigger a sync on {} now?", c.display_label))
                {
                    button type="submit" class="btn" { "Sync now" }
                }
            }
        }
    }
}

fn auth_kind_label(k: crate::providers::ProviderAuthKind) -> &'static str {
    use crate::providers::ProviderAuthKind::*;
    match k {
        OAuth2 => "oauth2",
        ApiKey => "api key",
        BankCoordinates => "bank coords",
        WalletPubkey => "wallet pubkey",
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('…');
    out
}
