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

/// HTMX 2.0.10 from jsdelivr with the SRI hash published in the official
/// docs. Bump together (URL + integrity must match the same build) or the
/// browser will refuse to execute the script.
const HTMX_SRC: &str =
    "https://cdn.jsdelivr.net/npm/htmx.org@2.0.10/dist/htmx.min.js";
const HTMX_SRI: &str =
    "sha384-H5SrcfygHmAuTDZphMHqBJLc3FhssKjG7w/CeCpFReSfwBWDTKpkzPP8c+cLsK+V";

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
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="dark light";
                title { (title) " — billing admin" }
                script src=(HTMX_SRC) integrity=(HTMX_SRI) crossorigin="anonymous" {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ConnectionStatus;
    use crate::scheduler::JobRunStatus;

    fn body() -> Markup {
        html! { p { "hello" } }
    }

    #[test]
    fn page_includes_htmx_script_with_sri() {
        let s = page("Dashboard", NavSection::Dashboard, body()).into_string();
        assert!(s.contains("<!DOCTYPE html>"));
        assert!(s.contains("htmx.org@2.0.10"));
        assert!(s.contains("integrity=\"sha384-H5SrcfygHmAuTDZphMHqBJLc"));
        assert!(s.contains("crossorigin=\"anonymous\""));
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

const STYLES: &str = r#"
:root {
  --bg: #0b0f15;
  --bg-elev: #11161f;
  --bg-elev-2: #161c27;
  --border: #1e2633;
  --border-strong: #2a3447;
  --text: #e6ebf5;
  --text-muted: #9aa6b8;
  --text-subtle: #6c7689;
  --accent: #8ab4ff;
  --accent-soft: rgba(138,180,255,0.15);
  --ok: #46d39a;
  --ok-soft: rgba(70,211,154,0.15);
  --warn: #f3c24c;
  --warn-soft: rgba(243,194,76,0.15);
  --fail: #f06c7d;
  --fail-soft: rgba(240,108,125,0.15);
  --muted: #6c7689;
  --muted-soft: rgba(108,118,137,0.18);
  --radius: 10px;
  --radius-sm: 6px;
  --shadow: 0 1px 0 rgba(255,255,255,0.03), 0 6px 24px rgba(0,0,0,0.25);
  --mono: ui-monospace, "JetBrains Mono", Menlo, "Cascadia Mono", Consolas, monospace;
  --sans: ui-sans-serif, -apple-system, "Segoe UI", Inter, system-ui, sans-serif;
}
@media (prefers-color-scheme: light) {
  :root {
    --bg: #f7f8fb;
    --bg-elev: #ffffff;
    --bg-elev-2: #fafbff;
    --border: #e3e7ef;
    --border-strong: #c6cdd9;
    --text: #14181f;
    --text-muted: #525c6b;
    --text-subtle: #7d8696;
    --accent: #2f6bff;
    --accent-soft: rgba(47,107,255,0.10);
    --ok: #128a5a;
    --ok-soft: rgba(18,138,90,0.10);
    --warn: #a87a05;
    --warn-soft: rgba(168,122,5,0.10);
    --fail: #c0364a;
    --fail-soft: rgba(192,54,74,0.10);
    --muted: #58616f;
    --muted-soft: rgba(88,97,111,0.10);
    --shadow: 0 1px 0 rgba(0,0,0,0.02), 0 4px 16px rgba(20,24,31,0.06);
  }
}

* { box-sizing: border-box; }
html, body { margin: 0; padding: 0; }
body {
  background: var(--bg);
  color: var(--text);
  font-family: var(--sans);
  font-size: 14px;
  line-height: 1.45;
  -webkit-font-smoothing: antialiased;
  text-rendering: optimizeLegibility;
}
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
code, kbd, pre { font-family: var(--mono); }
pre { background: var(--bg-elev-2); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 12px; overflow-x: auto; font-size: 12px; }

main { max-width: 1180px; margin: 24px auto 48px; padding: 0 24px; }

.nav { position: sticky; top: 0; z-index: 10; background: rgba(11,15,21,0.85); backdrop-filter: blur(8px); border-bottom: 1px solid var(--border); }
@media (prefers-color-scheme: light) { .nav { background: rgba(247,248,251,0.9); } }
.nav-inner { max-width: 1180px; margin: 0 auto; padding: 10px 24px; display: flex; gap: 18px; align-items: center; }
.brand { display: inline-flex; align-items: center; gap: 10px; color: var(--text); text-decoration: none; }
.brand:hover { text-decoration: none; }
.brand-mark { width: 28px; height: 28px; border-radius: 6px; background: linear-gradient(135deg, var(--accent), #5d8fff); color: #fff; display: inline-flex; align-items: center; justify-content: center; font-weight: 700; font-size: 12px; letter-spacing: 0.5px; }
.brand-text { display: flex; flex-direction: column; line-height: 1; }
.brand-title { font-weight: 600; font-size: 13px; }
.brand-sub { font-size: 10px; color: var(--text-subtle); text-transform: uppercase; letter-spacing: 1px; margin-top: 2px; }
.nav-links { display: flex; gap: 4px; margin-left: 8px; flex: 1; }
.nav-link { padding: 6px 10px; border-radius: var(--radius-sm); color: var(--text-muted); font-size: 13px; }
.nav-link:hover { color: var(--text); background: var(--bg-elev-2); text-decoration: none; }
.nav-link.is-active { color: var(--text); background: var(--accent-soft); }
.status-pill { display: inline-flex; align-items: center; gap: 6px; padding: 4px 10px; border: 1px solid var(--border); border-radius: 999px; font-size: 12px; color: var(--text-muted); }
.dot { width: 8px; height: 8px; border-radius: 50%; display: inline-block; }
.dot-ok { background: var(--ok); box-shadow: 0 0 0 3px var(--ok-soft); }
.dot-fail { background: var(--fail); box-shadow: 0 0 0 3px var(--fail-soft); }
.dot-pending { background: var(--warn); box-shadow: 0 0 0 3px var(--warn-soft); }

.foot { max-width: 1180px; margin: 32px auto 24px; padding: 12px 24px; color: var(--text-subtle); font-size: 11px; border-top: 1px solid var(--border); }

h1 { font-size: 22px; margin: 0 0 8px; letter-spacing: -0.01em; }
h2 { font-size: 16px; margin: 24px 0 8px; font-weight: 600; }
h3 { font-size: 14px; margin: 16px 0 6px; font-weight: 600; color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.06em; }
.caption { color: var(--text-muted); margin: 0; font-size: 12px; }

.section-head { display: flex; flex-direction: column; gap: 4px; margin: 12px 0 16px; }
.section-head h2 { margin: 0; }

.grid { display: grid; gap: 14px; }
.grid-stats { grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); }

.card { background: var(--bg-elev); border: 1px solid var(--border); border-radius: var(--radius); padding: 16px; box-shadow: var(--shadow); }
.stat { display: flex; flex-direction: column; gap: 6px; }
.stat-label { color: var(--text-muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; }
.stat-value { font-size: 26px; font-weight: 600; letter-spacing: -0.01em; font-variant-numeric: tabular-nums; }
.stat-hint { color: var(--text-subtle); font-size: 11px; }

.tabs { display: flex; gap: 4px; border-bottom: 1px solid var(--border); margin: 16px 0 0; flex-wrap: wrap; }
.tab { padding: 8px 12px; color: var(--text-muted); font-size: 13px; border-radius: var(--radius-sm) var(--radius-sm) 0 0; border-bottom: 2px solid transparent; }
.tab:hover { color: var(--text); text-decoration: none; }
.tab.is-active { color: var(--text); border-bottom-color: var(--accent); background: var(--accent-soft); }
.tab-panel { padding-top: 16px; }

.table-wrap { background: var(--bg-elev); border: 1px solid var(--border); border-radius: var(--radius); overflow: hidden; box-shadow: var(--shadow); }
table { width: 100%; border-collapse: collapse; font-size: 13px; }
thead th { text-align: left; padding: 10px 12px; background: var(--bg-elev-2); color: var(--text-muted); font-weight: 600; font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em; border-bottom: 1px solid var(--border); }
tbody td { padding: 10px 12px; border-top: 1px solid var(--border); vertical-align: top; }
tbody tr:hover { background: var(--bg-elev-2); }
tbody td.empty-row { text-align: center; color: var(--text-subtle); padding: 24px; font-style: italic; }
td.num, th.num { text-align: right; font-variant-numeric: tabular-nums; }
.short-id { background: var(--bg-elev-2); padding: 1px 6px; border-radius: 4px; border: 1px solid var(--border); font-size: 11px; }

.badge { display: inline-block; padding: 2px 8px; border-radius: 999px; font-size: 11px; font-weight: 500; letter-spacing: 0.01em; }
.badge-ok       { background: var(--ok-soft);    color: var(--ok); }
.badge-fail     { background: var(--fail-soft);  color: var(--fail); }
.badge-pending  { background: var(--warn-soft);  color: var(--warn); }
.badge-muted    { background: var(--muted-soft); color: var(--muted); }

button, .btn, input[type="submit"] {
  font-family: inherit; font-size: 12px; padding: 6px 10px; border-radius: var(--radius-sm);
  background: var(--bg-elev-2); color: var(--text); border: 1px solid var(--border-strong); cursor: pointer;
  transition: background 80ms ease, border-color 80ms ease;
}
button:hover, .btn:hover { background: var(--bg-elev); border-color: var(--accent); }
button.btn-primary, .btn.btn-primary { background: var(--accent); color: #0b0f15; border-color: var(--accent); font-weight: 600; }
button.btn-primary:hover, .btn.btn-primary:hover { filter: brightness(1.05); }
button.btn-ghost, .btn.btn-ghost { background: transparent; }
button.btn-danger, .btn.btn-danger { color: var(--fail); border-color: var(--fail); }
button[disabled], .btn[disabled] { opacity: 0.5; cursor: not-allowed; }
.btn-row { display: inline-flex; gap: 6px; }

form.inline { display: inline; }
form.stacked { display: grid; gap: 10px; }
label.field { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--text-muted); }
input[type="text"], input[type="email"], select, textarea {
  font-family: inherit; font-size: 13px; padding: 8px 10px; border-radius: var(--radius-sm);
  background: var(--bg-elev-2); color: var(--text); border: 1px solid var(--border); width: 100%;
}
input:focus, select:focus, textarea:focus { outline: 2px solid var(--accent-soft); border-color: var(--accent); }

.flash { margin-bottom: 12px; }
.flash:empty { display: none; }
.flash-error { background: var(--fail-soft); color: var(--fail); border: 1px solid var(--fail); padding: 8px 12px; border-radius: var(--radius-sm); font-size: 13px; }
.flash-ok    { background: var(--ok-soft);   color: var(--ok);   border: 1px solid var(--ok);   padding: 8px 12px; border-radius: var(--radius-sm); font-size: 13px; }

.kv { display: grid; grid-template-columns: max-content 1fr; gap: 8px 16px; font-size: 13px; }
.kv dt { color: var(--text-muted); font-weight: 500; }
.kv dd { margin: 0; font-variant-numeric: tabular-nums; }

.htmx-request .htmx-indicator { opacity: 1; }
.htmx-indicator { opacity: 0; transition: opacity 150ms ease; font-size: 11px; color: var(--text-subtle); }

.split { display: grid; grid-template-columns: 1fr; gap: 16px; }
@media (min-width: 960px) { .split { grid-template-columns: 1fr 1fr; } }

.row-actions { display: flex; gap: 6px; justify-content: flex-end; }
.muted { color: var(--text-subtle); }
.nowrap { white-space: nowrap; }
.tight { font-size: 12px; }
"#;
