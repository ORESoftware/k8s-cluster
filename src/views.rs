//! Customer portal HTML views (maud markup for the app shell, tabs, panels,
//! tables, and forms). Extracted from main.rs; `use super::*` inherits the
//! crate-root types (AppConfig, CustomerAuditEvent, CustomerCtx) and helpers
//! (encode_query_value, the *_fragment/table markup) these render with.
#![allow(clippy::too_many_lines)]

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CustomerTab {
    Dashboard,
    Auth,
    ApiKeys,
    Crons,
    Security,
    Activity,
    Notifications,
    Settings,
}

impl CustomerTab {
    pub(crate) fn all() -> [CustomerTab; 8] {
        [
            CustomerTab::Dashboard,
            CustomerTab::Auth,
            CustomerTab::ApiKeys,
            CustomerTab::Crons,
            CustomerTab::Security,
            CustomerTab::Activity,
            CustomerTab::Notifications,
            CustomerTab::Settings,
        ]
    }

    pub(crate) fn href(self) -> &'static str {
        match self {
            CustomerTab::Dashboard => "/app",
            CustomerTab::Auth => "/app/auth",
            CustomerTab::ApiKeys => "/app/api-keys",
            CustomerTab::Crons => "/app/crons",
            CustomerTab::Security => "/app/security",
            CustomerTab::Activity => "/app/activity",
            CustomerTab::Notifications => "/app/notifications",
            CustomerTab::Settings => "/app/settings",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            CustomerTab::Dashboard => "Dashboard",
            CustomerTab::Auth => "Account",
            CustomerTab::ApiKeys => "API Keys",
            CustomerTab::Crons => "Cron Jobs",
            CustomerTab::Security => "Security",
            CustomerTab::Activity => "Activity",
            CustomerTab::Notifications => "Notifications",
            CustomerTab::Settings => "Settings",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            CustomerTab::Dashboard => "Account posture, API access, preferences, and customer security in one workspace.",
            CustomerTab::Auth => "Your Supabase identity, verified organization membership, and isolated customer session.",
            CustomerTab::ApiKeys => "Create, rotate, scope, and audit customer API keys for production integrations.",
            CustomerTab::Crons => "Create schedules, attach managed code or webhooks, run jobs on demand, and inspect the traceable run trail.",
            CustomerTab::Security => "Two-factor authentication, trusted sessions, recovery, and account protection.",
            CustomerTab::Activity => "Organization-scoped account and API activity from the durable customer audit log.",
            CustomerTab::Notifications => "Key-rotation reminders, lock-contention alerts, and account notices delivered to you.",
            CustomerTab::Settings => "Preferences, notifications, default region, and team-level customer settings.",
        }
    }
}

pub(crate) fn customer_tab_href(tab: CustomerTab, org_id: &str) -> String {
    format!("{}?org_id={}", tab.href(), encode_query_value(org_id))
}

pub(crate) fn customer_page(
    config: &AppConfig,
    customer: &CustomerCtx,
    active: CustomerTab,
    org_id: &str,
    csrf_token: &str,
) -> Markup {
    let summary_href = format!(
        "/app/fragments/summary?org_id={}",
        encode_query_value(org_id)
    );
    let api_keys_href = customer_tab_href(CustomerTab::ApiKeys, org_id);
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="fiducia-customer-csrf" content=(csrf_token);
                title { "Fiducia Customer Portal" }
                link rel="stylesheet" href="/assets/customer.css";
                script src="/assets/htmx.min.js" defer {}
            }
            body {
                div class="app-shell" {
                    header class="topbar" {
                        div class="brand" {
                            div class="brand__mark" { "F" }
                            div class="brand__text" {
                                div class="brand__name" { "Fiducia Customer Portal" }
                                div class="brand__subdomain" { (config.customer_app_host) }
                            }
                        }
                        div class="topbar__status" {
                            span class="status-pill" data-status="online" { "verified" }
                            span class="status-pill" { (customer.email.as_deref().unwrap_or(&customer.user_id)) }
                            form method="post" action="/logout" {
                                input type="hidden" name="csrf_token" value=(csrf_token);
                                button type="submit" { "Sign out" }
                            }
                        }
                    }
                    main class="workspace" {
                        aside class="sidebar" {
                            section class="sidebar__section" {
                                p class="sidebar__label" { "Workspace" }
                                nav class="nav" aria-label="Customer portal" {
                                    @for tab in CustomerTab::all() {
                                        @let href = customer_tab_href(tab, org_id);
                                        @if tab == active {
                                            a href=(href) aria-current="page" {
                                                span { (tab.label()) }
                                            }
                                        } @else {
                                            a href=(href) {
                                                span { (tab.label()) }
                                            }
                                        }
                                    }
                                }
                            }
                            section class="sidebar__section" {
                                form class="region-select" method="get" action=(active.href()) {
                                    label class="sidebar__label" for="customer-org" { "Organization" }
                                    select id="customer-org" name="org_id" {
                                        @for available_org in &customer.orgs {
                                            option value=(available_org) selected[available_org == org_id] { (available_org) }
                                        }
                                    }
                                    button type="submit" { "Switch" }
                                }
                            }
                        }
                        section class="workspace__main" aria-labelledby="portal-title" {
                            div class="page-heading" {
                                div {
                                    h1 id="portal-title" { (active.label()) }
                                    p { (active.description()) }
                                }
                                div class="toolbar" {
                                    button type="button" hx-get=(summary_href) hx-target="#summary" hx-swap="innerHTML" { "Refresh" }
                                    a href=(api_keys_href) { "New API key" }
                                    a href="/api/info" { "API info" }
                                }
                            }
                            (customer_tab_content(config, customer, active, org_id, csrf_token))
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn customer_tab_content(
    config: &AppConfig,
    customer: &CustomerCtx,
    active: CustomerTab,
    org_id: &str,
    csrf_token: &str,
) -> Markup {
    match active {
        CustomerTab::Dashboard => dashboard_markup(config, customer, org_id),
        CustomerTab::Auth => auth_markup(customer, csrf_token),
        CustomerTab::ApiKeys => api_keys_markup(org_id, csrf_token),
        CustomerTab::Crons => cron_markup(org_id, csrf_token),
        CustomerTab::Security => security_markup(org_id),
        CustomerTab::Activity => activity_markup(org_id),
        CustomerTab::Notifications => notifications_markup(org_id),
        CustomerTab::Settings => settings_markup(org_id),
    }
}

pub(crate) fn dashboard_markup(config: &AppConfig, customer: &CustomerCtx, org_id: &str) -> Markup {
    let summary_href = format!(
        "/app/fragments/summary?org_id={}",
        encode_query_value(org_id)
    );
    html! {
        section id="summary" hx-get=(summary_href) hx-trigger="load, every 15s" hx-swap="innerHTML" {
            (summary_markup())
        }
        div class="panel-grid panel-grid--dashboard" {
            (auth_status_panel(config, customer, org_id))
            (api_key_summary_panel(org_id))
            (security_summary_panel(org_id))
            (preferences_summary_panel(org_id))
        }
    }
}

pub(crate) fn auth_status_panel(
    config: &AppConfig,
    customer: &CustomerCtx,
    org_id: &str,
) -> Markup {
    let supabase_state =
        if config.supabase_url.is_some() && config.supabase_publishable_key.is_some() {
            "configured"
        } else {
            "missing env"
        };
    let project_url = config.supabase_url.as_deref().unwrap_or("not configured");

    html! {
        section class="panel" aria-labelledby="auth-status-heading" {
            div class="panel__header" {
                h2 id="auth-status-heading" { "Supabase Auth" }
                span data-auth-status="" { "verified" }
            }
            div class="panel-body stack" {
                div class="identity-row" {
                    div {
                        p class="eyebrow" { "Customer session" }
                        p class="identity-row__primary" data-auth-email="" { (customer.email.as_deref().unwrap_or(&customer.user_id)) }
                    }
                    (status_tag(supabase_state))
                }
                p class="muted" { "Project: " span class="mono" { (project_url) } }
                div class="action-row" {
                    a class="button-link" href=(customer_tab_href(CustomerTab::Auth, org_id)) { "Account" }
                }
            }
        }
    }
}

pub(crate) fn api_key_summary_panel(org_id: &str) -> Markup {
    html! {
        section class="panel" aria-labelledby="api-key-summary-heading" {
            div class="panel__header" {
                h2 id="api-key-summary-heading" { "API Keys" }
                span { "Postgres-backed" }
            }
            div class="panel-body stack" {
                p class="muted" { "Issue scoped keys for customer workloads and rotate live keys without downtime." }
                dl class="detail-list" {
                    div {
                        dt { "Default scope" }
                        dd { "requests:write with idempotency keys" }
                    }
                    div {
                        dt { "Rotation" }
                        dd { "rotation reports the remaining edge/LB cache overlap" }
                    }
                }
                a class="button-link" href=(customer_tab_href(CustomerTab::ApiKeys, org_id)) { "Manage keys" }
            }
        }
    }
}

pub(crate) fn security_summary_panel(org_id: &str) -> Markup {
    html! {
        section class="panel" aria-labelledby="security-summary-heading" {
            div class="panel__header" {
                h2 id="security-summary-heading" { "Security" }
                span { "Supabase-managed" }
            }
            div class="panel-body stack" {
                p class="muted" { "Supabase owns MFA and passkey enrollment; trusted session records are loaded from the customer database." }
                dl class="detail-list" {
                    div {
                        dt { "Identity" }
                        dd { "verified by fiducia-auth" }
                    }
                    div {
                        dt { "Enrollment state" }
                        dd { "not guessed by this service" }
                    }
                }
                a class="button-link" href=(customer_tab_href(CustomerTab::Security, org_id)) { "Review security" }
            }
        }
    }
}

pub(crate) fn preferences_summary_panel(org_id: &str) -> Markup {
    html! {
        section class="panel" aria-labelledby="preferences-summary-heading" {
            div class="panel__header" {
                h2 id="preferences-summary-heading" { "Preferences" }
                span { "Postgres-backed" }
            }
            div class="panel-body stack" {
                p class="muted" { "Set default region, alert cadence, timezone, and customer-visible notifications." }
                p { "Values are rendered from the authenticated user's persisted row." }
                a class="button-link" href=(customer_tab_href(CustomerTab::Settings, org_id)) { "Open settings" }
            }
        }
    }
}

pub(crate) fn auth_markup(customer: &CustomerCtx, csrf_token: &str) -> Markup {
    html! {
        section class="panel" aria-labelledby="customer-session-heading" {
            div class="panel__header" {
                h2 id="customer-session-heading" { "Customer identity" }
                span class="status-pill" data-status="online" { "verified" }
            }
            div class="split-panel" {
                div class="session-box" {
                    p class="eyebrow" { "Supabase user" }
                    p class="identity-row__primary" { (customer.email.as_deref().unwrap_or(&customer.user_id)) }
                    p class="muted" { "Verified by fiducia-auth on this request." }
                }
                div class="session-box" {
                    p class="eyebrow" { "Organization membership" }
                    @for org in &customer.orgs {
                        code { (org) }
                    }
                }
            }
            form method="post" action="/logout" hx-post="/logout" {
                input type="hidden" name="csrf_token" value=(csrf_token);
                button type="submit" { "Sign out" }
            }
        }
    }
}

pub(crate) fn api_keys_markup(org_id: &str, csrf_token: &str) -> Markup {
    let fragment_href = format!(
        "/app/fragments/api-keys?org_id={}",
        encode_query_value(org_id)
    );
    html! {
        section class="panel" aria-labelledby="create-api-key-heading" {
            div class="panel__header" {
                h2 id="create-api-key-heading" { "Create API key" }
                span { "customer scoped" }
            }
            form class="form-grid form-grid--inline" method="post" action="/app/api-keys"
                hx-post="/app/api-keys" hx-target="#api-key-results" hx-swap="innerHTML" {
                input type="hidden" name="csrf_token" value=(csrf_token);
                input type="hidden" name="org_id" value=(org_id);
                input type="hidden" name="idempotency_key" value=(Uuid::new_v4().to_string());
                label {
                    span { "Name" }
                    input type="text" name="name" placeholder="Production checkout" required;
                }
                label {
                    span { "Environment" }
                    select name="environment" {
                        option value="live" { "Live" }
                        option value="test" { "Test" }
                    }
                }
                label {
                    span { "Scopes" }
                    select name="scope" {
                        option value="requests:read" { "requests:read" }
                        option value="requests:write" { "requests:write" }
                        option value="locks:read" { "locks:read" }
                        option value="locks:write" { "locks:write" }
                        option value="kv:read" { "kv:read" }
                        option value="kv:write" { "kv:write" }
                        option value="services:read" { "services:read" }
                        option value="services:write" { "services:write" }
                        option value="elections:read" { "elections:read" }
                        option value="elections:write" { "elections:write" }
                        option value="cron:read" { "cron:read" }
                        option value="cron:write" { "cron:write" }
                        option value="rate-limit:read" { "rate-limit:read" }
                        option value="rate-limit:write" { "rate-limit:write" }
                    }
                }
                button type="submit" { "Create key" }
            }
        }
        div id="api-key-results" hx-get=(fragment_href) hx-trigger="load" hx-swap="innerHTML" {
            p class="muted" { "Loading customer API keys…" }
        }
    }
}

pub(crate) fn security_markup(org_id: &str) -> Markup {
    let fragment_href = format!(
        "/app/fragments/security-sessions?org_id={}",
        encode_query_value(org_id)
    );
    html! {
        section class="panel" aria-labelledby="auth-security-heading" {
            div class="panel__header" {
                h2 id="auth-security-heading" { "Supabase account security" }
                span { "provider managed" }
            }
            p class="muted" {
                "MFA and passkeys are managed by Supabase Auth. This application does not display guessed enrollment state; production-key policy will only claim enforcement after fiducia-auth exposes a verified assurance level."
            }
        }
        div id="security-sessions" hx-get=(fragment_href) hx-trigger="load" hx-swap="innerHTML" {
            p class="muted" { "Loading trusted sessions…" }
        }
    }
}

pub(crate) fn activity_markup(org_id: &str) -> Markup {
    let fragment_href = format!(
        "/app/fragments/activity?org_id={}",
        encode_query_value(org_id)
    );
    html! {
        section class="panel" aria-labelledby="activity-heading" {
            div class="panel__header" {
                h2 id="activity-heading" { "Organization activity" }
                span { "audit log" }
            }
            p class="muted" {
                "Only records for the organization selected from your verified Supabase membership are shown. "
                "Network addresses, user agents, and internal audit metadata are never exposed here."
            }
        }
        div id="customer-activity" hx-get=(fragment_href) hx-trigger="load" hx-swap="innerHTML" {
            p class="muted" { "Loading organization activity…" }
        }
    }
}

pub(crate) fn notifications_markup(org_id: &str) -> Markup {
    let fragment_href = format!(
        "/app/fragments/notifications?org_id={}",
        encode_query_value(org_id)
    );
    html! {
        section class="panel" aria-labelledby="notifications-heading" {
            div class="panel__header" {
                h2 id="notifications-heading" { "Your notifications" }
                span { "account feed" }
            }
            p class="muted" {
                "Key-rotation reminders, lock-contention alerts, MFA nudges, and operator notices "
                "delivered to your account. Delivery preferences live under Settings."
            }
        }
        div id="customer-notifications" hx-get=(fragment_href) hx-trigger="load" hx-swap="innerHTML" {
            p class="muted" { "Loading notifications…" }
        }
    }
}

pub(crate) fn notifications_table_markup(
    notifications: &[entity::customer_notifications::Model],
    unread: u64,
    message: Option<&str>,
    csrf_token: &str,
) -> Markup {
    html! {
        section class="panel" aria-labelledby="notifications-table-heading" {
            div class="panel__header" {
                h2 id="notifications-table-heading" { "Recent notifications" }
                span { (unread) " unread / " (notifications.len()) " shown" }
            }
            @if let Some(message) = message {
                p class="inline-message" role="status" { (message) }
            }
            div class="table-wrap" {
                table {
                    thead {
                        tr {
                            th { "When" }
                            th { "Severity" }
                            th { "Notification" }
                            th { "State" }
                            th { "Action" }
                        }
                    }
                    tbody {
                        @if notifications.is_empty() {
                            tr { td colspan="5" class="muted" { "You have no notifications." } }
                        } @else {
                            @for note in notifications {
                                tr {
                                    td { (note.created_at.to_rfc3339()) }
                                    td { span class="status-pill" data-severity=(&note.severity) { (&note.severity) } }
                                    td {
                                        strong { (&note.title) }
                                        @if !note.body.is_empty() {
                                            div class="muted" { (&note.body) }
                                        }
                                        @if let Some(link) = &note.link {
                                            div { a href=(link) { "View" } }
                                        }
                                    }
                                    td { @if note.read_at.is_some() { "read" } @else { "unread" } }
                                    td {
                                        @if note.read_at.is_none() {
                                            form method="post" action="/app/notifications/read"
                                                hx-post="/app/notifications/read"
                                                hx-target="#customer-notifications"
                                                hx-swap="innerHTML" {
                                                input type="hidden" name="csrf_token" value=(csrf_token);
                                                input type="hidden" name="id" value=(note.id.to_string());
                                                button type="submit" { "Mark read" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn customer_activity_table_markup(events: &[CustomerAuditEvent]) -> Markup {
    html! {
        section class="panel" aria-labelledby="activity-table-heading" {
            div class="panel__header" {
                h2 id="activity-table-heading" { "Recent activity" }
                span { (events.len()) " shown" }
            }
            div class="table-wrap" {
                table {
                    thead {
                        tr {
                            th { "When" }
                            th { "Actor" }
                            th { "Action" }
                            th { "Target" }
                            th { "Request" }
                        }
                    }
                    tbody {
                        @if events.is_empty() {
                            tr { td colspan="5" class="muted" { "No customer-visible activity is recorded for this organization yet." } }
                        } @else {
                            @for event in events {
                                tr {
                                    td { (event.created_at) }
                                    td { (event.actor.as_deref().unwrap_or("system")) }
                                    td { code { (&event.action) } }
                                    td { (event.target.as_deref().unwrap_or("—")) }
                                    td { code { (event.request_id.as_deref().unwrap_or("—")) } }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn sessions_table_markup(
    sessions: &[fiducia_interfaces_db::customer::CustomerSessionsRow],
    message: Option<&str>,
    csrf_token: &str,
) -> Markup {
    html! {
        section class="panel" aria-labelledby="sessions-heading" {
            div class="panel__header" {
                h2 id="sessions-heading" { "Trusted sessions" }
                span { (sessions.len()) " recorded" }
            }
            @if let Some(message) = message {
                p class="inline-message" role="status" { (message) }
            }
            div class="table-wrap" {
                table {
                    thead {
                        tr {
                            th { "Device" }
                            th { "Location" }
                            th { "Last seen" }
                            th { "State" }
                            th { "Action" }
                        }
                    }
                    tbody {
                        @if sessions.is_empty() {
                            tr { td colspan="5" class="muted" { "No trusted sessions have been recorded." } }
                        } @else {
                            @for session in sessions {
                                tr {
                                    td { (&session.device) }
                                    td { (session.location.as_deref().unwrap_or("unknown")) }
                                    td { (session.last_seen.to_rfc3339()) }
                                    td { (&session.status) }
                                    td {
                                        @if session.status != "revoked" {
                                            form method="post" action="/app/security/sessions/revoke"
                                                hx-post="/app/security/sessions/revoke"
                                                hx-target="#security-sessions"
                                                hx-swap="innerHTML" {
                                                input type="hidden" name="csrf_token" value=(csrf_token);
                                                input type="hidden" name="device" value=(&session.device);
                                                button type="submit" { "Revoke" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn settings_markup(org_id: &str) -> Markup {
    let fragment_href = format!(
        "/app/fragments/preferences?org_id={}",
        encode_query_value(org_id)
    );
    html! {
        div id="customer-preferences" hx-get=(fragment_href) hx-trigger="load" hx-swap="innerHTML" {
            p class="muted" { "Loading persisted preferences…" }
        }
    }
}

pub(crate) fn preferences_form_markup(
    preferences: &CustomerPreferences,
    saved: bool,
    csrf_token: &str,
) -> Markup {
    html! {
        section class="panel" aria-labelledby="preferences-heading" {
            div class="panel__header" {
                h2 id="preferences-heading" { "Preferences" }
                span { "Postgres-backed" }
            }
            @if saved {
                p class="inline-message" role="status" { "Preferences saved." }
            }
            form class="settings-grid" method="post" action="/app/settings"
                hx-post="/app/settings" hx-target="#customer-preferences" hx-swap="innerHTML" {
                input type="hidden" name="csrf_token" value=(csrf_token);
                label class="form-field" {
                    span { "Default region" }
                    select name="region" {
                        @for region in CUSTOMER_REGIONS {
                            option value=(*region) selected[preferences.region == *region] { (*region) }
                        }
                    }
                }
                label class="form-field" {
                    span { "Timezone" }
                    input name="timezone" value=(&preferences.timezone) required;
                }
                label class="form-field" {
                    span { "Dashboard density" }
                    select name="density" {
                        option value="comfortable" selected[preferences.density == "comfortable"] { "Comfortable" }
                        option value="compact" selected[preferences.density == "compact"] { "Compact" }
                    }
                }
                fieldset class="toggle-group" {
                    legend { "Notifications" }
                    label class="checkbox-line" {
                        input type="checkbox" name="notify_lock_contention" value="1"
                            checked[preferences.notify_lock_contention];
                        span { "Lock contention" }
                    }
                    label class="checkbox-line" {
                        input type="checkbox" name="notify_key_rotation" value="1"
                            checked[preferences.notify_key_rotation];
                        span { "API key rotation" }
                    }
                    label class="checkbox-line" {
                        input type="checkbox" name="notify_mfa" value="1"
                            checked[preferences.notify_mfa];
                        span { "MFA changes" }
                    }
                }
                button type="submit" { "Save preferences" }
            }
        }
    }
}

pub(crate) fn summary_markup() -> Markup {
    html! {
        div class="summary-grid" {
            div class="metric" {
                p class="metric__label" { "API keys" }
                p class="metric__value" { "live" }
                p class="metric__hint" { "sanitized metadata from fiducia-auth after sign-in" }
            }
            div class="metric" {
                p class="metric__label" { "Preferences" }
                p class="metric__value" { "live" }
                p class="metric__hint" { "persisted per authenticated user" }
            }
            div class="metric" {
                p class="metric__label" { "Sessions" }
                p class="metric__value" { "live" }
                p class="metric__hint" { "trusted sessions from customer PostgreSQL" }
            }
            div class="metric" {
                p class="metric__label" { "Application boundary" }
                p class="metric__value" { "customer" }
                p class="metric__hint" { "operator infrastructure controls live only in the admin app" }
            }
        }
    }
}

pub(crate) fn status_tag(status: &str) -> Markup {
    html! {
        span class=(status_class(status)) { (status) }
    }
}

pub(crate) fn status_class(status: &str) -> &'static str {
    match status {
        "active" | "configured" | "enabled" | "held" | "current" | "healthy" | "committed"
        | "linearized" | "verified" => "tag tag--ok",
        "limited" | "missing env" | "pending" | "renewing" | "rotating" | "ttl" | "redirected" => {
            "tag tag--warn"
        }
        "blocked" | "degraded" | "disabled" | "expired" | "rejected" => "tag tag--error",
        _ => "tag tag--info",
    }
}
