//! Tenants: list, create (HTMX form post), and tenant-detail with tabs.

use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use maud::{Markup, html};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tenants::{CreateTenant, Tenant};

use super::{connections, jobs, locks, notifications};
use super::layout::{self, NavSection, Tab, caption, empty_row, is_htmx, section_header, tabs};
use super::time::rel;

const PAGE_LIMIT: i64 = 200;

pub async fn list_page(State(state): State<AppState>) -> AppResult<Markup> {
    let rows = state.tenants.list(PAGE_LIMIT).await.unwrap_or_default();

    let body = html! {
        h1 { "Tenants" }
        (caption(&format!("Most recent {} tenants.", PAGE_LIMIT)))

        div class="split" style="margin-top: 16px;" {
            section class="card" {
                h3 { "New tenant" }
                form
                    class="stacked"
                    hx-post="/admin/tenants"
                    hx-target="#tenants-table tbody"
                    hx-swap="afterbegin"
                    hx-on--after-request="if(event.detail.successful) this.reset();"
                {
                    label class="field" {
                        "Slug"
                        input type="text" name="slug" required="" placeholder="dancingdragons";
                    }
                    label class="field" {
                        "Display name"
                        input type="text" name="display_name" required="" placeholder="Dancing Dragons";
                    }
                    div style="display: grid; grid-template-columns: 1fr 1fr; gap: 10px;" {
                        label class="field" {
                            "Country"
                            input type="text" name="country_code" required="" placeholder="US" maxlength="2";
                        }
                        label class="field" {
                            "US state (optional)"
                            input type="text" name="us_state" placeholder="CA" maxlength="2";
                        }
                    }
                    label class="field" {
                        "Base currency"
                        input type="text" name="base_currency" value="USD" maxlength="3";
                    }
                    div class="btn-row" {
                        button type="submit" class="btn btn-primary" { "Create tenant" }
                        span class="htmx-indicator" { "creating…" }
                    }
                }
            }
            section {
                (section_header("All tenants", None))
                div #tenants-table class="table-wrap" {
                    table {
                        thead {
                            tr {
                                th { "Slug" }
                                th { "Display name" }
                                th { "Region" }
                                th { "Currency" }
                                th { "Status" }
                                th { "Created" }
                                th class="num" { "Open" }
                            }
                        }
                        tbody {
                            @if rows.is_empty() {
                                (empty_row(7, "No tenants yet. Create one on the left."))
                            }
                            @for t in &rows { (tenant_row(t)) }
                        }
                    }
                }
            }
        }
    };

    Ok(layout::page("Tenants", NavSection::Tenants, body))
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(input): Form<CreateTenantForm>,
) -> AppResult<Response> {
    let create = CreateTenant {
        slug: input.slug.trim().to_string(),
        display_name: input.display_name.trim().to_string(),
        country_code: input.country_code.trim().to_string(),
        us_state: input.us_state.and_then(non_empty),
        base_currency: input.base_currency.and_then(non_empty),
        kms_key_id: None,
    };
    if create.slug.is_empty() {
        return Err(AppError::BadRequest("slug must not be empty".into()));
    }

    let tenant = state.tenants.create(create).await?;

    // HTMX submit → return the new row to be prepended into <tbody>.
    if is_htmx(&headers) {
        return Ok(tenant_row(&tenant).into_response());
    }
    // Non-HTMX fallback: re-render the list page.
    list_page(State(state)).await.map(IntoResponse::into_response)
}

pub async fn detail_page(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<DetailQuery>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let tenant = state.tenants.by_id(id).await?;
    let active = q.tab.as_deref().and_then(Tab::from_slug).unwrap_or(Tab::Connections);

    // Inner content rendered server-side on first paint; HTMX swaps it for
    // subsequent tab clicks.
    let inner = render_tab(&state, &tenant, active).await;

    let body = html! {
        a href="/admin/tenants" class="muted tight" { "← back to tenants" }
        h1 style="margin-top: 8px;" { (tenant.display_name) }
        (caption(&format!(
            "slug: {}  ·  region: {}{}  ·  base currency: {}  ·  status: {}",
            tenant.slug,
            tenant.country_code,
            tenant.us_state.as_deref().map(|s| format!("/{s}")).unwrap_or_default(),
            tenant.base_currency,
            tenant.status,
        )))
        dl class="kv" style="margin-top: 12px;" {
            dt { "Tenant id" }   dd { code { (tenant.id) } }
            dt { "Created"    }  dd { (rel(tenant.created_at)) }
            dt { "KMS key"    }  dd { code { (tenant.kms_key_id) } }
        }
        (tabs(tenant.id, active, inner))
    };

    if is_htmx(&headers) {
        // Direct tab clicks land here via hx-get on /admin/tenants/{id}?tab=...
        // — return only the inner panel so HTMX swaps `#tab-panel` cleanly.
        return Ok(render_tab(&state, &tenant, active).await.into_response());
    }
    Ok(layout::page(&tenant.display_name, NavSection::Tenants, body).into_response())
}

async fn render_tab(state: &AppState, tenant: &Tenant, tab: Tab) -> Markup {
    match tab {
        Tab::Connections => connections::render_table(state, tenant.id).await,
        Tab::Jobs => jobs::render_table(state, tenant.id).await,
        Tab::Locks => locks::render_table(state, tenant.id).await,
        Tab::Notifications => notifications::render_panel(state, tenant.id).await,
    }
}

fn tenant_row(t: &Tenant) -> Markup {
    html! {
        tr {
            td { code { (t.slug) } }
            td { (t.display_name) }
            td {
                (t.country_code)
                @if let Some(s) = &t.us_state { (format!("/{s}")) }
            }
            td { (t.base_currency) }
            td { (status_badge(&t.status)) }
            td { (rel(t.created_at)) }
            td class="num" {
                a class="btn btn-ghost" href=(format!("/admin/tenants/{}", t.id)) { "open ›" }
            }
        }
    }
}

fn status_badge(status: &str) -> Markup {
    let class = match status {
        "active" => "badge badge-ok",
        "suspended" | "deleted" => "badge badge-fail",
        _ => "badge badge-muted",
    };
    html! { span class=(class) { (status) } }
}

fn non_empty(s: String) -> Option<String> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t.to_string()) }
}

#[derive(Debug, Deserialize)]
pub struct CreateTenantForm {
    pub slug: String,
    pub display_name: String,
    pub country_code: String,
    pub us_state: Option<String>,
    pub base_currency: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DetailQuery {
    pub tab: Option<String>,
}

impl Tab {
    fn from_slug(s: &str) -> Option<Self> {
        match s {
            "connections" => Some(Self::Connections),
            "jobs" => Some(Self::Jobs),
            "locks" => Some(Self::Locks),
            "notifications" => Some(Self::Notifications),
            _ => None,
        }
    }
}
