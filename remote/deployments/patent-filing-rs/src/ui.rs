use maud::{html, Markup, PreEscaped, DOCTYPE};

use crate::{types::PatentMatterPackage, HTMX_SRC, HTMX_SRI};

/// The home page shell. maud compile-checks the structure and auto-escapes any
/// dynamic value, replacing the previous `format!` string template. The pinned
/// htmx `src`/`integrity`/`crossorigin`/`referrerpolicy` wiring and every CSS
/// rule are preserved verbatim.
pub(crate) fn render_home() -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Patent Filing Workbench" }
                script src=(HTMX_SRC) integrity=(HTMX_SRI) crossorigin="anonymous" referrerpolicy="no-referrer" {}
                style { (PreEscaped(HOME_CSS)) }
            }
            body {
                header {
                    div class="top" {
                        h1 { "Patent Filing Workbench" }
                        div class="status-rail" {
                            span { "Intake" }
                            span { "Readiness" }
                            span { "Draft Package" }
                            span { "Patent Center Handoff" }
                        }
                    }
                }
                main {
                    section class="panel" {
                        div class="panel-head" {
                            h2 { "Invention Intake" }
                            a href="docs/api" { "API docs" }
                        }
                        form hx-post="ui/packages" hx-target="#package-output" hx-swap="innerHTML" hx-indicator="#package-spinner" {
                            div class="grid-two" {
                                label { "Title"
                                    input name="title" value="Adaptive thermal sensor array" required;
                                }
                                label { "Target filing"
                                    select name="target_filing" {
                                        option value="provisional" selected { "provisional" }
                                        option value="non-provisional" { "non-provisional" }
                                        option value="design" { "design" }
                                        option value="pct" { "pct" }
                                    }
                                }
                            }
                            div class="grid-two" {
                                label { "Inventors"
                                    textarea name="inventor_names" required { "Avery Chen\nMorgan Patel" }
                                }
                                label { "Applicant"
                                    input name="applicant" value="Example Robotics LLC";
                                }
                            }
                            label { "Technical field"
                                input name="technical_field" value="distributed sensing and thermal control";
                            }
                            label { "Summary"
                                textarea class="tall" name="invention_summary" required { "A distributed sensor array combines low-cost temperature probes, edge calibration, and a controller that changes sampling frequency based on local thermal gradients. Each node reports confidence and drift estimates so the controller can prioritize high-risk zones without flooding the network." }
                            }
                            label { "Problem"
                                textarea name="problem" required { "Existing thermal monitoring systems either sample too slowly to catch fast changes or sample every node constantly, which wastes network capacity and power in dense installations." }
                            }
                            label { "Solution"
                                textarea name="solution" required { "The array estimates local gradients at each node, assigns an adaptive sampling budget, and routes high-confidence alerts through a compact priority protocol while slower regions remain in a low-power cadence." }
                            }
                            div class="grid-two" {
                                label { "Novelty points"
                                    textarea name="novelty_claims" required { "Node-level drift confidence changes sampling rates\nGradient-triggered priority routing reduces bandwidth\nController fuses confidence scores with thermal risk zones" }
                                }
                                label { "Embodiments"
                                    textarea name="embodiments" { "Warehouse battery pack monitoring\nServer rack airflow diagnostics\nFactory motor enclosure monitoring" }
                                }
                            }
                            div class="grid-two" {
                                label { "Alternatives"
                                    textarea name="alternatives" { "Wireless mesh nodes\nWired industrial bus nodes\nCloud or local controller deployment" }
                                }
                                label { "Advantages"
                                    textarea name="advantages" { "Lower power usage\nReduced telemetry volume\nFaster high-risk thermal alerts" }
                                }
                            }
                            div class="grid-two" {
                                label { "Known prior art"
                                    textarea name="known_prior_art" { "Static threshold thermal monitoring systems\nUniform polling sensor networks" }
                                }
                                label { "Figures and evidence"
                                    textarea name="attachments" { "System block diagram\nSampling-state flow chart\nPrototype calibration notes" }
                                }
                            }
                            div class="grid-two" {
                                label { "Entity status"
                                    select name="entity_status" {
                                        option value="large" { "large" }
                                        option value="small" { "small" }
                                        option value="micro" selected { "micro" }
                                    }
                                }
                                label { "Public disclosure date"
                                    input name="public_disclosure_date" placeholder="YYYY-MM-DD";
                                }
                            }
                            div class="grid-two" {
                                label { "Provisional filing date"
                                    input name="provisional_filing_date" placeholder="YYYY-MM-DD";
                                }
                                label { "Foreign priority date"
                                    input name="foreign_priority_date" placeholder="YYYY-MM-DD";
                                }
                            }
                            div class="grid-two" {
                                label class="checkline" {
                                    input type="checkbox" name="attorney_review" checked;
                                    "Attorney review requested"
                                }
                            }
                            div class="actions" {
                                span id="package-spinner" class="htmx-indicator" { "Generating package..." }
                                button type="submit" { "Generate Filing Package" }
                            }
                        }
                    }
                    section class="panel" {
                        div class="panel-head" {
                            h2 { "Package Preview" }
                            a href="example" { "JSON example" }
                        }
                        div id="package-output" {
                            div class="placeholder" {
                                strong { "Pending intake" }
                                p { "The package preview will show readiness, draft sections, claim seeds, drawing plan, search plan, and filing handoff." }
                            }
                        }
                    }
                }
            }
        }
    }
}

const HOME_CSS: &str = r#":root {
      color-scheme: light;
      --bg: #f5f7f3;
      --ink: #172026;
      --muted: #5f6b73;
      --line: #cfd8d3;
      --panel: #ffffff;
      --green: #126d57;
      --blue: #294f7a;
      --red: #9f3f32;
      --gold: #9a6b18;
      --code: #eef2f0;
    }
    * { box-sizing: border-box; }
    body { margin: 0; background: var(--bg); color: var(--ink); font: 14px/1.45 ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    header { border-bottom: 1px solid var(--line); background: #ffffff; }
    .top { width: min(1240px, calc(100% - 28px)); margin: 0 auto; padding: 18px 0 14px; display: flex; justify-content: space-between; align-items: center; gap: 14px; }
    h1 { margin: 0; font-size: 22px; line-height: 1.1; letter-spacing: 0; }
    .status-rail { display: flex; gap: 8px; flex-wrap: wrap; color: var(--muted); font-size: 12px; }
    .status-rail span { border: 1px solid var(--line); background: #f9faf8; border-radius: 6px; padding: 5px 8px; }
    main { width: min(1240px, calc(100% - 28px)); margin: 16px auto 30px; display: grid; grid-template-columns: minmax(320px, 0.92fr) minmax(340px, 1.08fr); gap: 16px; align-items: start; }
    .panel { background: var(--panel); border: 1px solid var(--line); border-radius: 8px; overflow: hidden; }
    .panel-head { padding: 12px 14px; border-bottom: 1px solid var(--line); display: flex; justify-content: space-between; gap: 10px; align-items: center; }
    .panel-head h2 { margin: 0; font-size: 15px; letter-spacing: 0; }
    .panel-head a { color: var(--blue); text-decoration: none; font-size: 12px; }
    form { padding: 14px; display: grid; gap: 12px; }
    .grid-two { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }
    label { display: grid; gap: 5px; color: var(--muted); font-size: 12px; font-weight: 700; }
    input, textarea, select { width: 100%; border: 1px solid var(--line); border-radius: 6px; background: #fff; color: var(--ink); padding: 9px 10px; font: inherit; letter-spacing: 0; }
    textarea { min-height: 74px; resize: vertical; }
    textarea.tall { min-height: 118px; }
    .checkline { display: flex; gap: 8px; align-items: center; color: var(--ink); font-weight: 600; }
    .checkline input { width: auto; }
    .actions { display: flex; justify-content: flex-end; gap: 10px; align-items: center; border-top: 1px solid var(--line); padding-top: 12px; }
    button { border: 0; border-radius: 6px; background: var(--green); color: white; padding: 10px 14px; font-weight: 800; cursor: pointer; }
    button:hover { background: #0e5a48; }
    .htmx-indicator { opacity: 0; color: var(--muted); font-size: 12px; }
    .htmx-request .htmx-indicator, .htmx-request.htmx-indicator { opacity: 1; }
    #package-output { min-height: 520px; }
    .placeholder { color: var(--muted); padding: 18px; }
    .result { padding: 14px; }
    .result.error { border-left: 4px solid var(--red); }
    .score-row { display: grid; grid-template-columns: 110px 1fr; gap: 14px; align-items: center; margin-bottom: 12px; }
    .score { width: 96px; height: 96px; border: 8px solid var(--green); border-radius: 50%; display: grid; place-items: center; font-size: 24px; font-weight: 900; color: var(--green); }
    .badge { display: inline-flex; align-items: center; border-radius: 6px; padding: 4px 8px; font-size: 12px; font-weight: 800; background: #e9f4ef; color: var(--green); }
    .badge.warn { background: #fff4d9; color: var(--gold); }
    .badge.blocked { background: #fbe8e4; color: var(--red); }
    h3 { margin: 14px 0 7px; font-size: 13px; text-transform: uppercase; color: var(--muted); letter-spacing: 0; }
    ul { margin: 0; padding-left: 18px; }
    li { margin: 4px 0; }
    .columns { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
    .mini { border: 1px solid var(--line); border-radius: 8px; padding: 10px; background: #fbfcfb; }
    code { background: var(--code); border-radius: 5px; padding: 2px 5px; font-family: ui-monospace, "SFMono-Regular", Consolas, monospace; font-size: 12px; overflow-wrap: anywhere; }
    @media (max-width: 880px) {
      .top { align-items: flex-start; flex-direction: column; }
      main { grid-template-columns: 1fr; }
      .grid-two, .columns, .score-row { grid-template-columns: 1fr; }
      .score { width: 82px; height: 82px; font-size: 21px; }
    }"#;

/// The package-preview fragment swapped into `#package-output` by HTMX. maud
/// auto-escapes every interpolated value, so the previous per-field
/// `escape_html` calls (and their easy-to-forget failure mode) are gone.
pub(crate) fn render_package_fragment(package: &PatentMatterPackage) -> Markup {
    let readiness_class = if !package.readiness.blockers.is_empty() {
        "blocked"
    } else if package.readiness.score < 82 {
        "warn"
    } else {
        ""
    };
    let fee = &package.fee_estimate;
    let abstract_words = package
        .claim_audit
        .abstract_word_count
        .map(|count| format!("{count} words"))
        .unwrap_or_else(|| "n/a".to_string());
    let multi = if package.claim_audit.has_multiple_dependent_claim {
        " · multiple-dependent present"
    } else {
        ""
    };

    html! {
        div class="result" {
            div class="score-row" {
                div class="score" { (package.readiness.score) }
                div {
                    span class={ "badge " (readiness_class) } { (package.readiness.status) }
                    h2 { (package.title) }
                    p { code { (package.matter_id) } " · " (package.filing_track) }
                }
            }
            div class="columns" {
                div class="mini" {
                    h3 { "Blockers" }
                    ul {
                        @if package.readiness.blockers.is_empty() {
                            li { "No blockers detected." }
                        } @else {
                            @for item in &package.readiness.blockers {
                                li { (item.message) }
                            }
                        }
                    }
                }
                div class="mini" {
                    h3 { "Attorney Handoff" }
                    p { (package.attorney_handoff.summary) }
                }
            }
            h3 { "Draft Sections" }
            ul {
                @for section in package.draft.sections.iter().take(4) {
                    li { strong { (section.heading) } ": " (section.body.chars().take(220).collect::<String>()) }
                }
            }
            div class="columns" {
                div class="mini" {
                    h3 { "Claim Seeds" }
                    ul {
                        @for item in package.draft.claim_seeds.iter().take(5) {
                            li { (item) }
                        }
                    }
                }
                div class="mini" {
                    h3 { "Drawing Plan" }
                    ul {
                        @for item in package.draft.drawing_plan.iter().take(5) {
                            li { (item) }
                        }
                    }
                }
            }
            div class="columns" {
                div class="mini" {
                    h3 { "USPTO Fee Estimate (" (fee.entity) ", eff. " (fee.effective_date) ")" }
                    ul {
                        @for item in &fee.line_items {
                            li { (item.label) " · " (item.quantity) " × $" (format!("{:.0}", item.unit_usd)) " = " strong { "$" (format!("{:.0}", item.amount_usd)) } }
                        }
                    }
                    p { strong { "Estimated total: $" (format!("{:.0}", fee.total_usd)) " USD" } }
                }
                div class="mini" {
                    h3 { "Claim Audit · abstract " (abstract_words) }
                    p { (package.claim_audit.independent_claims) " independent / " (package.claim_audit.dependent_claims) " dependent / " (package.claim_audit.total_claims) " total" (multi) }
                    ul {
                        @if package.claim_audit.findings.is_empty() {
                            li { "No claim formality findings." }
                        } @else {
                            @for item in package.claim_audit.findings.iter().take(8) {
                                li { code { (item.severity) } " " (item.message) }
                            }
                        }
                    }
                }
            }
            h3 { "Filing Deadlines (today " (package.deadlines.today) ")" }
            ul {
                @if package.deadlines.milestones.is_empty() {
                    li { "No filing/disclosure/priority dates provided." }
                } @else {
                    @for item in &package.deadlines.milestones {
                        li { code { (item.status) } " " (item.label) " — due " (item.due_date) " (" (item.days_remaining) " days)" }
                    }
                }
            }
            h3 { "Search Queries" }
            ul {
                @for item in package.search_plan.queries.iter().take(5) {
                    li { code { (item.label) } " " (item.query) }
                }
            }
            h3 { "Filing Checklist" }
            ul {
                @for item in &package.filing_checklist {
                    li { strong { (item.label) } " " code { (item.status) } " - " (item.notes) }
                }
            }
        }
    }
}

#[cfg(test)]
mod maud_render_tests {
    use super::*;

    use crate::{package::build_package, state::Config, types::example_request};

    fn render_config() -> Config {
        Config {
            server_auth_secret: Some("secret".to_string()),
            allow_unauthenticated: false,
            patent_center_url: "https://patentcenter.uspto.gov/".to_string(),
            max_matters: 10,
            anthropic_api_key: None,
            anthropic_base_url: "https://api.anthropic.com".to_string(),
            ai_model: "claude-opus-4-8".to_string(),
            ai_max_concurrency: 4,
        }
    }

    #[test]
    fn home_page_renders_htmx_shell() {
        let html = render_home().into_string();
        // maud emits an uppercase DOCTYPE for the same document type.
        assert!(html.starts_with("<!DOCTYPE html>"));
        // Pinned htmx asset and its Subresource Integrity attributes are preserved verbatim.
        assert!(html.contains(
            "src=\"https://unpkg.com/htmx.org@1.9.12/dist/htmx.min.js\""
        ));
        assert!(html.contains(
            "integrity=\"sha384-ujb1lZYygJmzgSwoxRggbCHcjc0rB2XoQrxeTUQyRjrOnlCoYta87iKBWq3EsdM2\""
        ));
        assert!(html.contains("crossorigin=\"anonymous\""));
        assert!(html.contains("referrerpolicy=\"no-referrer\""));
        // HTMX wiring on the intake form is preserved verbatim.
        assert!(html.contains("hx-post=\"ui/packages\""));
        assert!(html.contains("hx-target=\"#package-output\""));
        assert!(html.contains("hx-swap=\"innerHTML\""));
        assert!(html.contains("hx-indicator=\"#package-spinner\""));
        // CSS embedded verbatim via PreEscaped.
        assert!(html.contains("--green: #126d57;"));
    }

    #[test]
    fn package_fragment_interpolates_dynamic_values() {
        let package = build_package(&render_config(), example_request())
            .expect("example intake should build a package");
        let html = render_package_fragment(&package).into_string();
        assert!(html.contains(&format!(
            "<div class=\"score\">{}</div>",
            package.readiness.score
        )));
        assert!(html.contains(&package.matter_id));
        assert!(html.contains("class=\"badge"));
    }

    #[test]
    fn package_fragment_auto_escapes_dynamic_values() {
        let mut request = example_request();
        request.title = "Sensor <script> & \"array\"".to_string();
        let package = build_package(&render_config(), request)
            .expect("intake should build a package");
        let html = render_package_fragment(&package).into_string();
        // maud auto-escapes the injected markup instead of emitting it raw.
        assert!(html.contains("Sensor &lt;script&gt; &amp; &quot;array&quot;"));
        assert!(!html.contains("<script>"));
    }
}
