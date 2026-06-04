//! Shared page chrome, design tokens, and small UI primitives.
//!
//! The CSS lives inline in the `<head>` to keep the admin UI fully
//! self-contained (one HTML response, one `<script>` for HTMX from CDN, no
//! bundler). Design tokens are CSS custom properties so the dark/light
//! scheme follows `prefers-color-scheme`.

use axum::http::HeaderMap;
use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::providers::ConnectionStatus;
use crate::scheduler::JobRunStatus;

use super::assets;

/// Tabs rendered on the tenant detail page. The selected tab also names the
/// HTMX fragment endpoint the tab targets.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Connections,
    Jobs,
    Locks,
    Notifications,
}

impl Tab {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Connections => "connections",
            Self::Jobs => "jobs",
            Self::Locks => "locks",
            Self::Notifications => "notifications",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Connections => "Connections",
            Self::Jobs => "Scheduled jobs",
            Self::Locks => "Leases",
            Self::Notifications => "Notifications",
        }
    }
    pub const ALL: [Tab; 4] = [
        Tab::Connections,
        Tab::Jobs,
        Tab::Locks,
        Tab::Notifications,
    ];
}

/// Top-level navigation entry.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NavSection {
    Dashboard,
    Tenants,
}

/// Returns true when the request was issued by HTMX (so we should return a
/// fragment rather than a full page).
pub fn is_htmx(headers: &HeaderMap) -> bool {
    headers
        .get("HX-Request")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Title shown in the browser tab + the top-of-page heading bar.
pub fn page(title: &str, section: NavSection, body: Markup) -> Markup {
    let htmx_src = assets::htmx_asset_path();
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="dark light";
                // Defense in depth alongside the X-Robots-Tag header set
                // by security::security_headers — covers tools that look
                // at <meta> but not response headers.
                meta name="robots" content="noindex, nofollow";
                meta name="referrer" content="same-origin";
                title { (title) " — billing admin" }
                // SRI integrity is enforced even for same-origin scripts;
                // any drift in the vendored bytes (caught at startup by
                // assets::verify_integrity) would surface here as a
                // browser-side refusal to execute.
                script
                    src=(htmx_src)
                    integrity=(assets::HTMX_INTEGRITY)
                    crossorigin="anonymous"
                    defer {}
                style { (PreEscaped(STYLES)) }
            }
            body hx-boost="true" hx-target-error="#flash" {
                (navbar(section))
                main {
                    div #flash class="flash" {}
                    (body)
                }
                (footer())
            }
        }
    }
}

fn navbar(section: NavSection) -> Markup {
    let is = |s: NavSection| if section == s { "is-active" } else { "" };
    html! {
        header class="nav" {
            div class="nav-inner" {
                a href="/admin" class="brand" {
                    span class="brand-mark" { "DD" }
                    span class="brand-text" {
                        span class="brand-title" { "billing-server" }
                        span class="brand-sub" { "admin" }
                    }
                }
                nav class="nav-links" {
                    a href="/admin"          class=(format!("nav-link {}", is(NavSection::Dashboard))) { "Dashboard" }
                    a href="/admin/tenants"  class=(format!("nav-link {}", is(NavSection::Tenants)))   { "Tenants" }
                    a href="/docs/api"       class="nav-link"  target="_blank" rel="noopener" { "API docs" }
                    a href="/metrics"        class="nav-link"  target="_blank" rel="noopener" { "Metrics" }
                }
                div
                    class="status-pill"
                    hx-get="/admin/status"
                    hx-trigger="load, every 5s"
                    hx-swap="innerHTML"
                {
                    span class="dot dot-pending" {}
                    span { "checking" }
                }
            }
        }
    }
}

fn footer() -> Markup {
    html! {
        footer class="foot" {
            "billing-server-rs · "
            (env!("CARGO_PKG_VERSION"))
            " · read-mostly admin · "
            a href="/healthz" target="_blank" rel="noopener" { "healthz" }
            " · "
            a href="/readyz" target="_blank" rel="noopener" { "readyz" }
        }
    }
}

/// Inline error banner. Use inside fragments that may fail.
pub fn flash_error(msg: impl AsRef<str>) -> Markup {
    html! {
        div class="flash flash-error" { (msg.as_ref()) }
    }
}

/// Subdued caption (used under cards and table headers).
pub fn caption(text: &str) -> Markup {
    html! { p class="caption" { (text) } }
}

/// Stat card for the dashboard.
pub fn stat_card(label: &str, value: impl AsRef<str>, hint: impl AsRef<str>) -> Markup {
    let hint = hint.as_ref();
    html! {
        div class="card stat" {
            span class="stat-label" { (label) }
            span class="stat-value" { (value.as_ref()) }
            @if !hint.is_empty() { span class="stat-hint" { (hint) } }
        }
    }
}

/// Page-level section header with a subtitle.
pub fn section_header(title: &str, sub: Option<&str>) -> Markup {
    html! {
        header class="section-head" {
            h2 { (title) }
            @if let Some(s) = sub { p class="caption" { (s) } }
        }
    }
}

/// Empty-state row to use inside `<tbody>` when a query returns zero rows.
pub fn empty_row(colspan: u8, msg: &str) -> Markup {
    html! {
        tr { td colspan=(colspan) class="empty-row" { (msg) } }
    }
}

/// Render a tab strip + the active content panel for the tenant detail page.
pub fn tabs(tenant_id: uuid::Uuid, active: Tab, content: Markup) -> Markup {
    html! {
        div class="tabs" {
            @for tab in Tab::ALL {
                a
                    href=(format!("/admin/tenants/{tenant_id}?tab={}", tab.slug()))
                    class=(if tab == active { "tab is-active" } else { "tab" })
                    hx-get=(format!("/admin/tenants/{tenant_id}/{}", tab.slug()))
                    hx-target="#tab-panel"
                    hx-push-url=(format!("/admin/tenants/{tenant_id}?tab={}", tab.slug()))
                {
                    (tab.label())
                }
            }
        }
        div #tab-panel class="tab-panel" {
            (content)
        }
    }
}

/// Colored pill for a generic connection status.
pub fn connection_status_badge(s: ConnectionStatus) -> Markup {
    let (class, text) = match s {
        ConnectionStatus::Active => ("badge badge-ok", "active"),
        ConnectionStatus::Pending => ("badge badge-pending", "pending"),
        ConnectionStatus::TokenRefreshFailed => ("badge badge-fail", "token failed"),
        ConnectionStatus::Revoked => ("badge badge-muted", "revoked"),
        ConnectionStatus::Expired => ("badge badge-fail", "expired"),
    };
    html! { span class=(class) { (text) } }
}

/// Colored pill for a scheduler job-run status.
pub fn job_run_status_badge(s: JobRunStatus) -> Markup {
    let (class, text) = match s {
        JobRunStatus::Pending => ("badge badge-pending", "pending"),
        JobRunStatus::Claimed => ("badge badge-pending", "running"),
        JobRunStatus::Succeeded => ("badge badge-ok", "succeeded"),
        JobRunStatus::Failed => ("badge badge-fail", "failed"),
        JobRunStatus::DeadLettered => ("badge badge-fail", "dead-lettered"),
        JobRunStatus::Cancelled => ("badge badge-muted", "cancelled"),
    };
    html! { span class=(class) { (text) } }
}

/// On/Off badge for a scheduled job.
pub fn enabled_badge(enabled: bool) -> Markup {
    if enabled {
        html! { span class="badge badge-ok" { "enabled" } }
    } else {
        html! { span class="badge badge-muted" { "disabled" } }
    }
}

/// Monospace short id (first 8 chars of a UUID). Hover shows the full id.
pub fn short_id<T: std::fmt::Display>(id: T) -> Markup {
    let s = id.to_string();
    let short = s.chars().take(8).collect::<String>();
    html! { code class="short-id" title=(s) { (short) } }
}

// Inline CSS for the admin surface. Kept at module bottom so the test
// module is contiguous with the public items above it (clippy lint
// `items_after_test_module`). The CSS is *trusted* static text — Maud
// wraps it in `PreEscaped` so the literal `<style>` block ships verbatim.
const STYLES: &str = include_str!("./layout.css");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ConnectionStatus;
    use crate::scheduler::JobRunStatus;

    fn body() -> Markup {
        html! { p { "hello" } }
    }

    #[test]
    fn page_includes_self_hosted_htmx_with_sri() {
        let s = page("Dashboard", NavSection::Dashboard, body()).into_string();
        assert!(s.contains("<!DOCTYPE html>"));
        // Self-hosted, hash-pinned URL — no CDN reference.
        assert!(s.contains("src=\"/admin/static/htmx-"));
        assert!(!s.contains("cdn.jsdelivr.net"));
        assert!(!s.contains("unpkg.com"));
        // SRI hash still present so the browser refuses to execute
        // tampered bytes even when served from our own origin.
        assert!(s.contains("integrity=\"sha384-H5SrcfygHmAuTDZphMHqBJLc"));
        assert!(s.contains("crossorigin=\"anonymous\""));
        // Anti-indexing belt-and-suspenders.
        assert!(s.contains(r#"name="robots" content="noindex, nofollow""#));
        assert!(s.contains("Dashboard — billing admin"));
        assert!(s.contains("hello"));
    }

    #[test]
    fn navbar_marks_active_section() {
        let dash = page("d", NavSection::Dashboard, body()).into_string();
        let ten = page("t", NavSection::Tenants, body()).into_string();
        assert!(dash.contains(r#"href="/admin" class="nav-link is-active""#));
        assert!(ten.contains(r#"href="/admin/tenants" class="nav-link is-active""#));
    }

    #[test]
    fn is_htmx_detects_header() {
        let mut h = HeaderMap::new();
        assert!(!is_htmx(&h));
        h.insert("HX-Request", "true".parse().unwrap());
        assert!(is_htmx(&h));
        h.insert("HX-Request", "TRUE".parse().unwrap());
        assert!(is_htmx(&h));
    }

    #[test]
    fn status_badges_match_classes() {
        for (s, expect) in [
            (ConnectionStatus::Active, "badge-ok"),
            (ConnectionStatus::Pending, "badge-pending"),
            (ConnectionStatus::TokenRefreshFailed, "badge-fail"),
            (ConnectionStatus::Revoked, "badge-muted"),
            (ConnectionStatus::Expired, "badge-fail"),
        ] {
            let html = connection_status_badge(s).into_string();
            assert!(html.contains(expect), "{s:?} -> {html}");
        }
        for (s, expect) in [
            (JobRunStatus::Pending, "pending"),
            (JobRunStatus::Claimed, "running"),
            (JobRunStatus::Succeeded, "succeeded"),
            (JobRunStatus::Failed, "failed"),
            (JobRunStatus::DeadLettered, "dead-lettered"),
            (JobRunStatus::Cancelled, "cancelled"),
        ] {
            let html = job_run_status_badge(s).into_string();
            assert!(html.contains(expect), "{s:?} -> {html}");
        }
    }

    #[test]
    fn tabs_render_all_four_with_active_one_highlighted() {
        let t = uuid::Uuid::nil();
        let s = tabs(t, Tab::Jobs, html! { p { "inner" } }).into_string();
        assert!(s.contains("Connections"));
        assert!(s.contains("Scheduled jobs"));
        assert!(s.contains("Leases"));
        assert!(s.contains("Notifications"));
        // Only the jobs tab should be highlighted as active.
        assert_eq!(s.matches(r#"class="tab is-active""#).count(), 1);
        assert!(s.contains(r#"class="tab is-active""#));
        // HTMX wiring: the tabs should target the panel and push URL.
        assert!(s.contains(r##"hx-target="#tab-panel""##));
        assert!(s.contains("hx-push-url="));
        assert!(s.contains("inner"));
    }

    #[test]
    fn short_id_keeps_first_eight_and_full_in_title() {
        let id = uuid::Uuid::parse_str("0123456789abcdef0123456789abcdef").unwrap();
        let s = short_id(id).into_string();
        assert!(s.contains("01234567"));
        assert!(s.contains(r#"title="01234567-89ab-cdef-0123-456789abcdef""#));
    }
}

