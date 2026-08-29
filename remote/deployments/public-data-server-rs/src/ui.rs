use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::Ordering,
};

use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde_json::Value;

use crate::catalog::source_catalog;
use crate::state::AppState;
use crate::types::AuthFailure;

pub(crate) async fn root() -> Html<String> {
    Html(render_root().into_string())
}

/// Landing page. maud compile-checks the structure and auto-escapes any dynamic
/// value, replacing the previous hand-written HTML string literal.
pub(crate) fn render_root() -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "dd-public-data-server" }
                style { (PreEscaped(ROOT_CSS)) }
            }
            body {
                main {
                    h1 { "dd-public-data-server" }
                    p { "Rust public-data ingestion service for webhooks, scraper orchestration, public/government source normalization, grant matching, trend/correlation graph data, white-paper evidence briefs, and Spark/Airflow pipeline job intents." }
                    div class="actions" {
                        a class="button" href="./ui" { "Operator UI" }
                        a class="link" href="./docs/api" { "API docs" }
                    }
                    div class="grid" {
                        div class="card" { strong { "Sources" } p { code { "GET /sources" } } }
                        div class="card" { strong { "Ingest" } p { code { "POST /ingest" } " and " code { "POST /webhooks/ingest" } } }
                        div class="card" { strong { "Analysis" } p { code { "POST /analysis/trends" } ", " code { "/analysis/correlations" } } }
                        div class="card" { strong { "Docs" } p { code { "GET /docs/api" } } }
                    }
                }
            }
        }
    }
}

pub(crate) const ROOT_CSS: &str = r#"body { margin: 0; font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: #172026; background: #f7f8fb; }
    main { max-width: 1040px; margin: 0 auto; padding: 40px 24px; }
    h1 { margin: 0 0 8px; font-size: 32px; letter-spacing: 0; }
    p { line-height: 1.5; max-width: 780px; }
    .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 12px; margin-top: 24px; }
    .card { background: white; border: 1px solid #d8dde6; border-radius: 8px; padding: 16px; }
    .actions { display: flex; flex-wrap: wrap; gap: 10px; margin-top: 20px; }
    .button { color: white; background: #1f6f70; border-radius: 6px; padding: 10px 14px; text-decoration: none; font-weight: 700; }
    .link { color: #1f5f87; font-weight: 700; }
    code { background: #eef1f5; border-radius: 4px; padding: 2px 5px; }"#;

/// The operator dashboard shell. HTMX wiring (hx-get/hx-post/hx-target/hx-swap/
/// hx-trigger/hx-disabled-elt) and the exact CSS are preserved; the initial
/// fragment bodies are embedded as already-rendered maud `Markup`.
pub(crate) fn render_ui_shell(state: &AppState) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Public Data Operations" }
                script src="https://unpkg.com/htmx.org@2.0.4/dist/htmx.min.js" defer {}
                style { (PreEscaped(UI_SHELL_CSS)) }
            }
            body {
                header {
                    div {
                        p class="kicker" { "Public data" }
                        h1 { "Evidence Operations" }
                        p class="muted" { "Scrapes, webhooks, grants, model evidence, and pipeline handoffs." }
                    }
                    nav aria-label="Public data navigation" {
                        a href="./" { "Home" }
                        a href="./docs/api" { "API docs" }
                        a href="./metrics" { "Metrics" }
                    }
                }
                main {
                    section id="summary" hx-get="./ui/fragments/summary" hx-trigger="load, every 15s" hx-swap="innerHTML" {
                        (render_ui_summary(state))
                    }
                    div class="grid split" {
                        section class="panel" {
                            h2 { "Scrape Intake" }
                            form hx-post="./ui/actions/scrape" hx-target="#scrape-result" hx-swap="innerHTML" hx-disabled-elt="button" {
                                label { "Source"
                                    input name="source" value="sbir" autocomplete="off";
                                }
                                label { "Public URL"
                                    input name="url" type="url" value="https://www.sbir.gov/" required;
                                }
                                label { "Dataset"
                                    input name="dataset_id" value="sbir-opportunities" autocomplete="off";
                                }
                                label { "Strategy"
                                    select name="strategy" {
                                        option value="auto" { "auto" }
                                        option value="native-fetch" { "native-fetch" }
                                        option value="cheerio" { "cheerio" }
                                        option value="browser" { "browser" }
                                    }
                                }
                                label { "Selector"
                                    input name="selector" autocomplete="off" placeholder="main";
                                }
                                label { "Tags"
                                    input name="tags" value="grants, public-data" autocomplete="off";
                                }
                                div class="checks" {
                                    label class="check" { input type="checkbox" name="include_links" checked; " Links" }
                                    label class="check" { input type="checkbox" name="render_javascript"; " JavaScript" }
                                    label class="check" { input type="checkbox" name="pipeline_enabled" checked; " Pipeline" }
                                }
                                button class="primary" type="submit" { "Run Scrape" }
                            }
                            div id="scrape-result" class="result" aria-live="polite" {}
                        }
                        section id="sources" class="panel" hx-get="./ui/fragments/sources" hx-trigger="load" hx-swap="innerHTML" {
                            (render_ui_sources())
                        }
                    }
                    section id="recent-records" class="panel" style="margin-top:16px" hx-get="./ui/fragments/recent-records" hx-trigger="load, every 20s" hx-swap="innerHTML" {
                        (render_ui_recent_records(state))
                    }
                }
            }
        }
    }
}

pub(crate) const UI_SHELL_CSS: &str = r#":root { color-scheme: light; --ink: #172026; --muted: #5d6975; --line: #d8dde6; --panel: #ffffff; --page: #f5f7f8; --teal: #1f6f70; --blue: #1f5f87; --amber: #9a6615; --danger: #a43d3d; }
    * { box-sizing: border-box; }
    body { margin: 0; font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: var(--ink); background: var(--page); }
    header, main { max-width: 1180px; margin: 0 auto; padding: 0 24px; }
    header { display: flex; align-items: flex-end; justify-content: space-between; gap: 20px; padding-top: 28px; padding-bottom: 18px; border-bottom: 1px solid var(--line); }
    h1, h2, h3, p { margin-top: 0; letter-spacing: 0; }
    h1 { margin-bottom: 4px; font-size: 28px; }
    h2 { font-size: 18px; margin-bottom: 14px; }
    h3 { font-size: 15px; margin-bottom: 6px; }
    p { line-height: 1.5; }
    nav { display: flex; flex-wrap: wrap; gap: 10px; justify-content: flex-end; }
    nav a, button, .button { min-height: 38px; display: inline-flex; align-items: center; justify-content: center; border-radius: 6px; border: 1px solid var(--line); padding: 8px 12px; color: var(--ink); background: #ffffff; text-decoration: none; font: inherit; font-weight: 700; cursor: pointer; }
    button.primary { color: #ffffff; background: var(--teal); border-color: var(--teal); }
    button:disabled { opacity: 0.6; cursor: wait; }
    main { padding-top: 22px; padding-bottom: 44px; }
    .kicker { margin-bottom: 4px; color: var(--teal); font-size: 12px; font-weight: 800; text-transform: uppercase; }
    .grid { display: grid; gap: 14px; }
    .stats { grid-template-columns: repeat(auto-fit, minmax(170px, 1fr)); }
    .split { grid-template-columns: minmax(300px, 420px) 1fr; align-items: start; margin-top: 16px; }
    .panel, .stat { background: var(--panel); border: 1px solid var(--line); border-radius: 8px; padding: 16px; }
    .stat strong { display: block; font-size: 26px; }
    .muted { color: var(--muted); }
    .status-row { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 10px; margin-top: 12px; }
    .badge, .pill { display: inline-flex; align-items: center; border-radius: 999px; padding: 3px 8px; font-size: 12px; font-weight: 800; background: #e9f2f2; color: #15595a; }
    .badge.warn { background: #fff1d6; color: var(--amber); }
    .badge.danger { background: #f8dddd; color: var(--danger); }
    .pill { margin: 2px 4px 2px 0; background: #eef1f5; color: #35414d; }
    form { display: grid; gap: 12px; }
    label { display: grid; gap: 6px; font-size: 13px; font-weight: 800; color: #35414d; }
    input, select { width: 100%; min-height: 38px; border: 1px solid var(--line); border-radius: 6px; padding: 8px 10px; font: inherit; background: #ffffff; color: var(--ink); }
    .checks { display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); gap: 8px; }
    .check { display: flex; align-items: center; gap: 8px; font-weight: 700; }
    .check input { width: 16px; min-height: 16px; }
    .result { min-height: 48px; }
    .notice { border: 1px solid var(--line); border-left: 4px solid var(--teal); border-radius: 8px; padding: 12px; background: #ffffff; }
    .notice.error { border-left-color: var(--danger); }
    table { width: 100%; border-collapse: collapse; font-size: 14px; }
    th, td { padding: 10px 8px; text-align: left; border-bottom: 1px solid var(--line); vertical-align: top; }
    th { color: #35414d; font-size: 12px; text-transform: uppercase; }
    code { background: #eef1f5; border-radius: 4px; padding: 2px 5px; }
    .empty { border: 1px dashed var(--line); border-radius: 8px; color: var(--muted); padding: 14px; background: #ffffff; }
    @media (max-width: 820px) { header { align-items: flex-start; flex-direction: column; } nav { justify-content: flex-start; } .split { grid-template-columns: 1fr; } }"#;

pub(crate) fn render_stat(label: &str, value: impl ToString, note: &str) -> Markup {
    html! {
        div class="stat" {
            span class="muted" { (label) }
            strong { (value.to_string()) }
            span class="muted" { (note) }
        }
    }
}

pub(crate) fn render_badge(label: &str, tone: &str) -> Markup {
    html! {
        span class={ "badge " (tone) } { (label) }
    }
}

pub(crate) fn render_ui_summary(state: &AppState) -> Markup {
    let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
    let dataset_count = store
        .records
        .iter()
        .map(|record| record.dataset_id.clone())
        .collect::<BTreeSet<_>>()
        .len();
    let grant_count = store
        .records
        .iter()
        .filter(|record| record.grant.is_some())
        .count();
    let last_analysis = store.analyses.last().cloned();
    let last_job = store.pipeline_jobs.last().cloned();
    let record_count = store.records.len();
    let webhook_count = store.webhook_receipts.len();
    let analysis_count = store.analyses.len();
    let job_count = store.pipeline_jobs.len();
    drop(store);

    let nats = if state.nats.is_some() {
        render_badge("NATS connected", "")
    } else {
        render_badge("NATS disabled", "warn")
    };
    let auth = if state.config.allow_unauthenticated {
        render_badge("auth relaxed", "warn")
    } else if state.config.server_auth_secret.is_some() {
        render_badge("auth configured", "")
    } else {
        render_badge("auth missing", "danger")
    };
    html! {
        div class="grid stats" {
            (render_stat("Records", record_count, "bounded in-process ledger"))
            (render_stat("Datasets", dataset_count, "normalized collections"))
            (render_stat("Grants", grant_count, "records with funding data"))
            (render_stat("Webhooks", webhook_count, "provider receipts"))
            (render_stat("Analyses", analysis_count, "trend/correlation/brief outputs"))
            (render_stat("Jobs", job_count, "pipeline intents"))
        }
        div class="status-row" {
            div class="panel" {
                h3 { "Runtime" }
                (nats) " " (auth) " "
                p class="muted" {
                    "NATS messages: " (state.metrics.nats_messages_total.load(Ordering::Relaxed))
                    "; published: " (state.metrics.nats_published_total.load(Ordering::Relaxed))
                }
            }
            div class="panel" {
                h3 { "Last Analysis" }
                @if let Some(analysis) = last_analysis {
                    div { strong { (analysis.kind) } p class="muted" { (analysis.summary) } }
                } @else {
                    div class="muted" { "No analyses yet." }
                }
            }
            div class="panel" {
                h3 { "Last Pipeline Job" }
                @if let Some(job) = last_job {
                    div { strong { (job.job_type) } p class="muted" { code { (job.job_id) } " " (job.status) } }
                } @else {
                    div class="muted" { "No pipeline jobs yet." }
                }
            }
        }
    }
}

pub(crate) fn source_str(source: &Value, key: &str) -> String {
    source
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

pub(crate) fn render_ui_sources() -> Markup {
    html! {
        h2 { "Source Catalog" }
        table {
            thead { tr { th { "Source" } th { "Base URL" } th { "Strategy" } th { "Notes" } } }
            tbody {
                @for source in source_catalog() {
                    tr {
                        td {
                            strong { (source_str(&source, "name")) }
                            div class="muted" { (source_str(&source, "kind")) }
                        }
                        td { (source_str(&source, "baseUrl")) }
                        td { code { (source_str(&source, "defaultStrategy")) } }
                        td { (source_str(&source, "notes")) }
                    }
                }
            }
        }
    }
}

pub(crate) fn render_pills(values: &[String]) -> Markup {
    html! {
        @if values.is_empty() {
            span class="muted" { "none" }
        } @else {
            @for value in values.iter().take(8) {
                span class="pill" { (value) }
            }
        }
    }
}

pub(crate) fn render_metric_pills(metrics: &BTreeMap<String, f64>) -> Markup {
    html! {
        @if metrics.is_empty() {
            span class="muted" { "none" }
        } @else {
            @for (key, value) in metrics.iter().take(8) {
                span class="pill" { (key) " " (format!("{value:.2}")) }
            }
        }
    }
}

pub(crate) fn render_ui_recent_records(state: &AppState) -> Markup {
    let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
    html! {
        h2 { "Recent Records" }
        @if store.records.is_empty() {
            div class="empty" { "No records yet." }
        } @else {
            table {
                thead { tr { th { "Record" } th { "Dataset" } th { "Source" } th { "Tags" } th { "Metrics" } } }
                tbody {
                    @for record in store.records.iter().rev().take(12) {
                        @let title = record.title.as_deref().unwrap_or(&record.record_id);
                        tr {
                            td {
                                strong { (title) }
                                div class="muted" { code { (record.record_id) } }
                            }
                            td { (record.dataset_id) }
                            td { (record.source) }
                            td { (render_pills(&record.tags)) }
                            td { (render_metric_pills(&record.metrics)) }
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn render_ui_notice(title: &str, detail: &str, error: bool) -> Markup {
    let class_name = if error { "notice error" } else { "notice" };
    html! {
        div class=(class_name) {
            strong { (title) }
            p class="muted" { (detail) }
        }
    }
}

pub(crate) fn render_ui_scrape_result(value: &Value) -> Markup {
    let request_id = value
        .get("requestId")
        .and_then(Value::as_str)
        .unwrap_or("scrape");
    let dataset_id = value
        .get("datasetId")
        .and_then(Value::as_str)
        .unwrap_or("dataset");
    let record_title = value
        .pointer("/record/title")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/record/recordId").and_then(Value::as_str))
        .unwrap_or("record");
    let status = value
        .pointer("/scraper/status")
        .and_then(Value::as_u64)
        .map(|status| status.to_string())
        .unwrap_or_else(|| "ok".to_string());
    html! {
        div class="notice" {
            strong { "Scrape accepted" }
            p class="muted" {
                code { (request_id) } " stored " code { (record_title) } " in " code { (dataset_id) } "; scraper status " (status) "."
            }
            button type="button" hx-get="./ui/fragments/summary" hx-target="#summary" hx-swap="innerHTML" { "Refresh Board" }
        }
    }
}

pub(crate) fn ui_auth_failure_response(state: &AppState, failure: AuthFailure) -> Response {
    state
        .metrics
        .auth_failures_total
        .fetch_add(1, Ordering::Relaxed);
    let message = match failure {
        AuthFailure::MissingSecret => "server auth secret is not configured",
        AuthFailure::Unauthorized => "unauthorized",
    };
    (
        StatusCode::UNAUTHORIZED,
        Html(render_ui_notice("Unauthorized", message, true).into_string()),
    )
        .into_response()
}
