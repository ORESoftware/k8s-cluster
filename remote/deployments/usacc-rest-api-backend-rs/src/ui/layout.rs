//! Shared page chrome, design tokens, and small UI primitives for the
//! `/app` operator console.
//!
//! The CSS lives inline in the `<head>` so the console is one
//! self-contained HTML response plus one vendored `<script>` for HTMX — no
//! bundler. Every URL the console emits is built through [`Ui::url`] so the
//! same binary renders correct links both directly (base `""`) and behind
//! the path-stripping gateway (base `/usacc`).

use maud::{html, Markup, PreEscaped, DOCTYPE};

use super::assets;

/// Per-request rendering context: the external base-path prefix the
/// console is reached through. Cheap to clone (borrowed from `AppState`).
#[derive(Clone, Copy)]
pub struct Ui<'a> {
    pub base: &'a str,
}

impl<'a> Ui<'a> {
    pub fn new(base: &'a str) -> Self {
        Self { base }
    }

    /// Build an absolute URL for `suffix` (which starts with `/app...`),
    /// prefixed with the configured base path.
    pub fn url(&self, suffix: &str) -> String {
        format!("{}{}", self.base, suffix)
    }
}

/// Top-level navigation entry; also names the page heading.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NavSection {
    Dashboard,
    Cases,
    Users,
    Elections,
    Simulations,
}

/// Full HTML document shell.
pub fn page(ui: Ui<'_>, title: &str, section: NavSection, body: Markup) -> Markup {
    let htmx_src = ui.url(&assets::htmx_asset_suffix());
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="dark light";
                meta name="robots" content="noindex, nofollow";
                meta name="referrer" content="same-origin";
                title { (title) " — USACC console" }
                script
                    src=(htmx_src)
                    integrity=(assets::HTMX_INTEGRITY)
                    crossorigin="anonymous"
                    defer {}
                style { (PreEscaped(STYLES)) }
            }
            body hx-boost="true" hx-target-error="#flash" {
                (navbar(ui, section))
                main {
                    div #flash class="flash" {}
                    (body)
                }
                (footer())
            }
        }
    }
}

fn navbar(ui: Ui<'_>, section: NavSection) -> Markup {
    let cls = |s: NavSection| {
        if section == s {
            "nav-link is-active"
        } else {
            "nav-link"
        }
    };
    html! {
        header class="nav" {
            div class="nav-inner" {
                a href=(ui.url("/app")) class="brand" {
                    span class="brand-mark" { "US" }
                    span class="brand-text" {
                        span class="brand-title" { "Anti-Corruption Court" }
                        span class="brand-sub" { "console" }
                    }
                }
                nav class="nav-links" {
                    a href=(ui.url("/app"))             class=(cls(NavSection::Dashboard))   { "Dashboard" }
                    a href=(ui.url("/app/cases"))       class=(cls(NavSection::Cases))       { "Cases" }
                    a href=(ui.url("/app/users"))       class=(cls(NavSection::Users))       { "Users" }
                    a href=(ui.url("/app/elections"))   class=(cls(NavSection::Elections))   { "Elections" }
                    a href=(ui.url("/app/simulations")) class=(cls(NavSection::Simulations)) { "Simulations" }
                    a href=(ui.url("/docs/api")) class="nav-link" target="_blank" rel="noopener" { "API docs" }
                }
                div
                    class="status-pill"
                    hx-get=(ui.url("/app/status"))
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
            "usacc-rest-api-backend-rs · "
            (env!("CARGO_PKG_VERSION"))
            " · operator console · "
            a href="/healthz" target="_blank" rel="noopener" { "healthz" }
            " · "
            a href="/metrics" target="_blank" rel="noopener" { "metrics" }
        }
    }
}

/// Inline error banner. Use inside fragments that may fail.
pub fn flash_error(msg: impl AsRef<str>) -> Markup {
    html! { div class="flash flash-error" { (msg.as_ref()) } }
}

/// Inline success banner.
pub fn flash_ok(msg: impl AsRef<str>) -> Markup {
    html! { div class="flash flash-ok" { (msg.as_ref()) } }
}

/// Subdued caption.
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

/// Page-level section header with an optional subtitle.
pub fn section_header(title: &str, sub: Option<&str>) -> Markup {
    html! {
        header class="section-head" {
            h2 { (title) }
            @if let Some(s) = sub { p class="caption" { (s) } }
        }
    }
}

/// Empty-state row for use inside `<tbody>`.
pub fn empty_row(colspan: u8, msg: &str) -> Markup {
    html! { tr { td colspan=(colspan) class="empty-row" { (msg) } } }
}

/// Colored status pill. `kind` picks the palette.
pub fn status_badge(value: &str) -> Markup {
    let class = match value {
        "active" | "succeeded" | "certified" | "resolved" | "open" => "badge badge-ok",
        "pending" | "draft" | "running" | "screening" | "inquiry" | "signature_collection"
        | "admission_review" | "trial" | "appeal" => "badge badge-pending",
        "failed" | "banned" | "suspended" | "canceled" | "cancelled" => "badge badge-fail",
        _ => "badge badge-muted",
    };
    html! { span class=(class) { (value) } }
}

/// Monospace short id (first 8 chars). Hover shows the full id.
pub fn short_id(id: &str) -> Markup {
    let short: String = id.chars().take(8).collect();
    html! { code class="short-id" title=(id) { (short) } }
}

const STYLES: &str = include_str!("./layout.css");

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> Markup {
        html! { p { "hello" } }
    }

    #[test]
    fn page_self_hosts_htmx_with_sri() {
        let ui = Ui::new("");
        let s = page(ui, "Dashboard", NavSection::Dashboard, body()).into_string();
        assert!(s.contains("<!DOCTYPE html>"));
        assert!(s.contains("src=\"/app/static/htmx-"));
        assert!(!s.contains("cdn.jsdelivr.net"));
        assert!(s.contains("integrity=\"sha384-H5SrcfygHmAuTDZphMHqBJLc"));
        assert!(s.contains(r#"name="robots" content="noindex, nofollow""#));
        assert!(s.contains("Dashboard — USACC console"));
        assert!(s.contains("hello"));
    }

    #[test]
    fn base_path_prefixes_every_url() {
        let ui = Ui::new("/usacc");
        let s = page(ui, "d", NavSection::Cases, body()).into_string();
        assert!(s.contains("src=\"/usacc/app/static/htmx-"));
        assert!(s.contains(r#"href="/usacc/app/cases""#));
        assert!(s.contains(r#"hx-get="/usacc/app/status""#));
    }

    #[test]
    fn navbar_marks_active_section() {
        let ui = Ui::new("");
        let cases = page(ui, "c", NavSection::Cases, body()).into_string();
        assert!(cases.contains(r#"href="/app/cases" class="nav-link is-active""#));
    }
}
