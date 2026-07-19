//! Server-rendered Maud views and htmx fragments.
//!
//! The page shell loads htmx (with the websocket extension) and defines the
//! visual system once. Handlers return either a full [`page`] or a bare
//! [`Markup`] fragment; htmx swaps fragments in place without a full navigation,
//! and the same fragment renderers feed the websocket push in `ws.rs` — one
//! source of truth for "what a run row looks like".

use dd_pg_defs_sea_orm::{fab_plans, fab_runs};
use maud::{html, Markup, DOCTYPE};

/// htmx + its websocket extension, pinned by version. Served from the page
/// rather than a CDN so the Content-Security-Policy can stay `self`-only.
pub(crate) const HTMX_VERSION: &str = "1.9.12";

const CSS: &str = r#"
:root { color-scheme: light dark; --bg:#0b1020; --panel:#141a30; --line:#26304f;
  --ink:#e6e9f2; --muted:#9aa4c0; --accent:#7aa2ff; --ok:#7fe0a7; --warn:#ffd479; --bad:#ff8f8f; }
* { box-sizing: border-box; }
body { margin:0; min-height:100vh; background:var(--bg); color:var(--ink);
  font:16px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif; }
header { padding:1rem 1.5rem; border-bottom:1px solid var(--line); display:flex; align-items:baseline; gap:1rem; }
header .brand { letter-spacing:.08em; font-weight:700; color:var(--accent); }
header .who { margin-left:auto; color:var(--muted); font-size:.85rem; }
main { max-width:60rem; margin:0 auto; padding:1.5rem; }
h1 { font-size:1.3rem; margin:.2rem 0 1rem; }
a { color:var(--accent); text-decoration:none; }
a:hover { text-decoration:underline; }
.card { background:var(--panel); border:1px solid var(--line); border-radius:12px; padding:1rem 1.25rem; margin:.75rem 0; }
.row { display:flex; gap:1rem; align-items:center; }
.muted { color:var(--muted); }
.badge { font-size:.72rem; padding:.15rem .5rem; border-radius:999px; border:1px solid var(--line); }
.badge.additive { color:var(--accent); }
.badge.subtractive { color:var(--warn); }
.badge.hybrid { color:var(--ok); }
.status-queued { color:var(--muted); }
.status-running { color:var(--accent); }
.status-succeeded { color:var(--ok); }
.status-failed, .status-aborted { color:var(--bad); }
table { width:100%; border-collapse:collapse; font-size:.9rem; }
th, td { text-align:left; padding:.4rem .5rem; border-bottom:1px solid var(--line); }
.empty { color:var(--muted); padding:2rem 0; text-align:center; }
"#;

/// Full HTML document shell.
pub(crate) fn page(title: &str, operator_email: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Daedalus — " (title) }
                style { (maud::PreEscaped(CSS)) }
                // Local, versioned; the CSP forbids third-party origins.
                script src={ "/assets/htmx-" (HTMX_VERSION) ".min.js" } {}
                script src={ "/assets/htmx-ws-" (HTMX_VERSION) ".min.js" } {}
            }
            body {
                header {
                    span class="brand" { "DAEDALUS" }
                    span class="muted" { "fabrication planning" }
                    span class="who" { (operator_email) }
                }
                main { (body) }
            }
        }
    }
}

/// The landing list of a caller's plans.
pub(crate) fn plan_list(plans: &[fab_plans::Model]) -> Markup {
    html! {
        h1 { "Your fabrication plans" }
        @if plans.is_empty() {
            p class="empty" { "No plans yet." }
        } @else {
            @for plan in plans {
                a href={ "/plans/" (plan.id) } {
                    div class="card" {
                        div class="row" {
                            strong { (plan.title) }
                            span class={ "badge " (plan.process_family) } { (plan.process_family) }
                            span class="muted" style="margin-left:auto" {
                                span class={ "status-" (plan.status) } { (plan.status) }
                            }
                        }
                        div class="muted" { (truncate(&plan.goal, 140)) }
                    }
                }
            }
        }
    }
}

/// A single plan's detail page. The runs table live-updates over a websocket
/// using htmx's ws extension — the server pushes a replacement `#runs` fragment.
pub(crate) fn plan_detail(plan: &fab_plans::Model, runs: &[fab_runs::Model]) -> Markup {
    html! {
        p { a href="/" { "← all plans" } }
        h1 { (plan.title) }
        div class="row" {
            span class={ "badge " (plan.process_family) } { (plan.process_family) }
            span class={ "status-" (plan.status) } { (plan.status) }
        }
        p class="muted" { (plan.goal) }
        section hx-ext="ws" ws-connect={ "/plans/" (plan.id) "/runs/ws" } {
            (runs_fragment(runs))
        }
    }
}

/// The `#runs` fragment, rendered both on first paint and on every websocket
/// push. `id="runs"` is what htmx's ws swap targets.
pub(crate) fn runs_fragment(runs: &[fab_runs::Model]) -> Markup {
    html! {
        div id="runs" {
            h1 style="font-size:1rem" { "Runs" }
            @if runs.is_empty() {
                p class="empty" { "No runs yet." }
            } @else {
                table {
                    thead { tr { th { "machine" } th { "status" } th { "progress" } th { "started" } } }
                    tbody {
                        @for run in runs {
                            tr {
                                td { (run.machine_id) }
                                td { span class={ "status-" (run.status) } { (run.status) } }
                                td { (run.progress) "%" }
                                td class="muted" { (run.started_at.map(|t| t.to_rfc3339()).unwrap_or_else(|| "—".into())) }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Byte-safe truncation on a char boundary, with an ellipsis when cut.
fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_char_boundaries_and_only_adds_ellipsis_when_cut() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exactly-ten", 11), "exactly-ten");
        // Multi-byte characters must not be split mid-codepoint (would panic).
        let s = "ααααα";
        assert_eq!(truncate(s, 3), "ααα…");
    }

    #[test]
    fn empty_plan_list_renders_the_empty_state() {
        let rendered = plan_list(&[]).into_string();
        assert!(rendered.contains("No plans yet."));
    }

    #[test]
    fn runs_fragment_is_targetable_by_htmx_ws() {
        // The ws swap replaces the element with id="runs"; if this id ever
        // changes, live updates silently stop, so pin it with a test.
        let rendered = runs_fragment(&[]).into_string();
        assert!(rendered.contains(r#"id="runs""#));
        assert!(rendered.contains("No runs yet."));
    }
}
