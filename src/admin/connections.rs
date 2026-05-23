//! Provider connections table + per-connection HTMX actions.

use axum::extract::{Path, State};
use maud::{Markup, html};
use uuid::Uuid;

use crate::error::AppResult;
use crate::providers::connection::ProviderConnection;
use crate::state::AppState;

use super::layout::{connection_status_badge, empty_row, flash_error, section_header, short_id};
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
        Err(e) => return flash_error(e.to_string()),
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
                    @for c in &rows { (connection_row(c)) }
                }
            }
        }
    }
}

pub async fn sync_now(
    State(state): State<AppState>,
    Path(conn_id): Path<Uuid>,
) -> AppResult<Markup> {
    // The connection knows its own tenant, so we don't need a separate path
    // segment for it here. Look it up to get tenant_id + region.
    let row = sqlx::query_scalar::<_, Uuid>(
        r#"SELECT tenant_id FROM provider_connections WHERE id = $1"#,
    )
    .bind(conn_id)
    .fetch_one(&state.pool)
    .await?;

    let tenant = state.tenants.by_id(row).await?;
    let region = tenant.region()?;

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

    let conn = state.connections.get(tenant.id, conn_id).await?;
    Ok(connection_row(&conn))
}

fn connection_row(c: &ProviderConnection) -> Markup {
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
                    hx-post=(format!("/admin/connections/{}/sync", c.id))
                    hx-target=(format!("#conn-{}", c.id))
                    hx-swap="outerHTML"
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
