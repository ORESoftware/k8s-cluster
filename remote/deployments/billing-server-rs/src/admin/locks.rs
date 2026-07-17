//! Leases (`tenant_locks`) for a tenant.

use axum::extract::{Path, State};
use maud::{Markup, html};
use uuid::Uuid;

use crate::state::AppState;

use super::errors;
use super::layout::{empty_row, section_header, short_id};
use super::time::rel;

pub async fn table_fragment(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
) -> Markup {
    render_table(&state, tenant_id).await
}

pub(super) async fn render_table(state: &AppState, tenant_id: Uuid) -> Markup {
    let rows = match state.locks.list(tenant_id).await {
        Ok(r) => r,
        Err(e) => return errors::sanitized("list leases", &e),
    };

    html! {
        (section_header(
            "Active leases",
            Some("Tenant-scoped locks (`tenant_locks`). Expired rows persist until the sweeper job runs."),
        ))
        div class="table-wrap" {
            table {
                thead {
                    tr {
                        th { "Resource" }
                        th { "Token" }
                        th { "Holder" }
                        th { "Acquired" }
                        th { "Expires" }
                        th { "State" }
                    }
                }
                tbody {
                    @if rows.is_empty() {
                        (empty_row(6, "No active leases for this tenant."))
                    }
                    @for l in &rows {
                        tr {
                            td { code { (l.resource_key) } }
                            td { (short_id(l.lease_token)) }
                            td {
                                @match l.holder.as_deref() {
                                    Some(h) => (h),
                                    None => span class="muted" { "—" },
                                }
                            }
                            td { (rel(l.acquired_at)) }
                            td { (rel(l.expires_at)) }
                            td {
                                @if l.expired {
                                    span class="badge badge-muted" { "expired" }
                                } @else {
                                    span class="badge badge-ok" { "held" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
