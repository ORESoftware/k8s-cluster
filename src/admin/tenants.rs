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

use super::layout::{self, NavSection, Tab, caption, empty_row, is_htmx, section_header, tabs};
use super::time::rel;
use super::validation;
use super::{connections, jobs, locks, notifications};

const PAGE_LIMIT: i64 = 200;

pub async fn list_page(State(state): State<AppState>) -> AppResult<Markup> {
    let rows = state.tenants.list(PAGE_LIMIT).await.unwrap_or_default();

    let body = html! {
        h1 { "Tenants" }
        (caption(&format!("Most recent {} tenants.", PAGE_LIMIT)))

        div class="split" style="margin-top: 16px;" {
            section class="card" {
                h3 { "New tenant" }
                p class="muted tight" {
                    "Every tenant must be created with its first Shared Auth owner. "
                    "Use the canonical Shared Auth subject, not an email address."
                }
                form
                    class="stacked"
                    hx-post="/admin/tenants"
                    hx-target="#tenants-table tbody"
                    hx-swap="afterbegin"
                {
                    label class="field" {
                        "Slug"
                        input
                            type="text"
                            name="slug"
                            required=""
                            placeholder="dancingdragons"
                            pattern="[a-z][a-z0-9-]{2,39}"
                            minlength="3"
                            maxlength="40"
                            autocomplete="off"
                            spellcheck="false";
                    }
                    label class="field" {
                        "Display name"
                        input
                            type="text"
                            name="display_name"
                            required=""
                            placeholder="Dancing Dragons"
                            maxlength="120"
                            autocomplete="off";
                    }
                    label class="field" {
                        "Initial Shared Auth owner subject"
                        input
                            type="text"
                            name="owner_shared_user_id"
                            required=""
                            placeholder="shared-auth-user-id"
                            minlength="1"
                            maxlength="200"
                            autocomplete="off"
                            spellcheck="false";
                    }
                    div style="display: grid; grid-template-columns: 1fr 1fr; gap: 10px;" {
                        label class="field" {
                            "Country"
                            input
                                type="text"
                                name="country_code"
                                required=""
                                placeholder="US"
                                minlength="2"
                                maxlength="2"
                                pattern="[A-Za-z]{2}"
                                autocomplete="off"
                                spellcheck="false";
                        }
                        label class="field" {
                            "US state (optional)"
                            input
                                type="text"
                                name="us_state"
                                placeholder="CA"
                                maxlength="2"
                                pattern="[A-Za-z]{2}"
                                autocomplete="off"
                                spellcheck="false";
                        }
                    }
                    label class="field" {
                        "Base currency"
                        input
                            type="text"
                            name="base_currency"
                            value="USD"
                            minlength="3"
                            maxlength="3"
                            pattern="[A-Za-z]{3}"
                            autocomplete="off"
                            spellcheck="false";
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
    let slug = input.slug.trim().to_lowercase();
    let display_name = input.display_name.trim().to_string();
    let owner_shared_user_id = input.owner_shared_user_id.trim().to_string();
    let country_code = input.country_code.trim().to_uppercase();
    let us_state = input.us_state.and_then(non_empty).map(|s| s.to_uppercase());
    let base_currency = input
        .base_currency
        .and_then(non_empty)
        .map(|s| s.to_uppercase());

    validation::slug(&slug).map_err(|message| AppError::BadRequest(message.into()))?;
    validation::display_name(&display_name)
        .map_err(|message| AppError::BadRequest(message.into()))?;
    validation::country_code(&country_code)
        .map_err(|message| AppError::BadRequest(message.into()))?;
    if let Some(state_code) = us_state.as_deref() {
        validation::us_state(state_code).map_err(|message| AppError::BadRequest(message.into()))?;
    }
    if let Some(currency) = base_currency.as_deref() {
        validation::currency_code(currency)
            .map_err(|message| AppError::BadRequest(message.into()))?;
    }

    let create = CreateTenant {
        slug: slug.clone(),
        display_name,
        country_code,
        us_state,
        base_currency,
        kms_key_id: None,
    };
    let tenant = state
        .tenants
        .create_owned(create, &owner_shared_user_id)
        .await?;

    tracing::info!(
        admin.action = "tenant.create",
        admin.tenant_id = %tenant.id,
        admin.tenant_slug = %tenant.slug,
        auth.owner_subject = %owner_shared_user_id,
        "admin: tenant and initial Shared Auth owner created atomically"
    );

    if is_htmx(&headers) {
        return Ok(tenant_row(&tenant).into_response());
    }
    list_page(State(state))
        .await
        .map(IntoResponse::into_response)
}

pub async fn detail_page(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<DetailQuery>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let tenant = state.tenants.by_id(id).await?;
    let active = query
        .tab
        .as_deref()
        .and_then(Tab::from_slug)
        .unwrap_or(Tab::Connections);

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

fn tenant_row(tenant: &Tenant) -> Markup {
    html! {
        tr {
            td { code { (tenant.slug) } }
            td { (tenant.display_name) }
            td {
                (tenant.country_code)
                @if let Some(state_code) = &tenant.us_state { (format!("/{state_code}")) }
            }
            td { (tenant.base_currency) }
            td { (status_badge(&tenant.status)) }
            td { (rel(tenant.created_at)) }
            td class="num" {
                a class="btn btn-ghost" href=(format!("/admin/tenants/{}", tenant.id)) { "open ›" }
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

fn non_empty(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateTenantForm {
    pub slug: String,
    pub display_name: String,
    pub owner_shared_user_id: String,
    pub country_code: String,
    pub us_state: Option<String>,
    pub base_currency: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DetailQuery {
    pub tab: Option<String>,
}

impl Tab {
    fn from_slug(value: &str) -> Option<Self> {
        match value {
            "connections" => Some(Self::Connections),
            "jobs" => Some(Self::Jobs),
            "locks" => Some(Self::Locks),
            "notifications" => Some(Self::Notifications),
            _ => None,
        }
    }
}
