use std::sync::{Arc, RwLock};

use dd_nats_subject_defs::{
    PUBLIC_DATA_ANALYSIS_RESULTS_SUBJECT, PUBLIC_DATA_INGEST_REQUESTS_QUEUE_GROUP,
    PUBLIC_DATA_INGEST_REQUESTS_SUBJECT, PUBLIC_DATA_INGEST_RESULTS_SUBJECT,
    PUBLIC_DATA_PIPELINE_JOBS_SUBJECT, PUBLIC_DATA_WEBHOOK_EVENTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
use serde_json::json;

use crate::analysis::{correlation_summaries, trend_summaries};
use crate::grants::grant_matches_from_records;
use crate::state::{AppState, Config, Metrics, PublicDataStore};
use crate::types::{DataRecord, GrantMatchRequest, GrantOpportunity, UiScrapeForm};
use crate::ui::{render_root, render_ui_notice, render_ui_scrape_result, render_ui_shell};
use crate::util::validate_public_url;


fn test_record(
    record_id: &str,
    dataset_id: &str,
    source: &str,
    tags: &[&str],
    metrics: &[(&str, f64)],
    grant: Option<GrantOpportunity>,
) -> DataRecord {
    DataRecord {
        record_id: record_id.to_string(),
        dataset_id: dataset_id.to_string(),
        source: source.to_string(),
        source_url: Some("https://www.sbir.gov/".to_string()),
        title: Some(record_id.to_string()),
        summary: Some("energy public data analytics grant research".to_string()),
        published_at: Some("2026-01-01".to_string()),
        collected_at_ms: 1,
        authors: Vec::new(),
        tags: tags.iter().map(|tag| tag.to_string()).collect(),
        metrics: metrics
            .iter()
            .map(|(name, value)| (name.to_string(), *value))
            .collect(),
        grant,
        raw: None,
    }
}

#[test]
fn public_url_validation_blocks_private_local_and_credential_targets() {
    for url in [
        "http://localhost/data",
        "https://127.0.0.1/data",
        "https://10.2.3.4/data",
        "https://172.16.1.2/data",
        "https://192.168.1.2/data",
        "https://[::1]/data",
        "https://[fc00::1]/data",
        "https://user@example.gov/data",
        "ftp://data.gov/file",
    ] {
        assert!(validate_public_url(url).is_err(), "{url} should be blocked");
    }
    assert!(validate_public_url("https://www.data.gov/").is_ok());
    assert!(validate_public_url("https://pubmed.ncbi.nlm.nih.gov/").is_ok());
}

#[test]
fn trend_and_correlation_summaries_detect_linear_relationships() {
    let records = vec![
        test_record(
            "r1",
            "science",
            "pubmed",
            &["science"],
            &[("citations", 1.0), ("funding", 2.0)],
            None,
        ),
        test_record(
            "r2",
            "science",
            "pubmed",
            &["science"],
            &[("citations", 2.0), ("funding", 4.0)],
            None,
        ),
        test_record(
            "r3",
            "science",
            "pubmed",
            &["science"],
            &[("citations", 3.0), ("funding", 6.0)],
            None,
        ),
    ];
    let trends = trend_summaries(&records, &Some(vec!["citations".to_string()]));
    assert_eq!(trends.len(), 1);
    assert_eq!(trends[0].direction, "up");
    assert!((trends[0].slope_per_record - 1.0).abs() < 1e-9);

    let correlations = correlation_summaries(&records, &None);
    let strongest = correlations
        .iter()
        .find(|item| {
            (item.left_metric == "citations" && item.right_metric == "funding")
                || (item.left_metric == "funding" && item.right_metric == "citations")
        })
        .expect("expected citations/funding correlation");
    assert!((strongest.pearson - 1.0).abs() < 1e-9);
    assert_eq!(strongest.strength, "very-strong");
}

#[test]
fn grant_matching_respects_focus_terms_and_minimum_amount() {
    let strong_grant = GrantOpportunity {
        grant_id: Some("sbir-energy-ai".to_string()),
        title: "Energy AI public data grant".to_string(),
        agency: Some("DOE".to_string()),
        program: Some("SBIR".to_string()),
        amount: Some(250_000.0),
        due_date: Some("2026-09-15".to_string()),
        eligibility: Some("US small business research teams".to_string()),
        topics: vec![
            "energy".to_string(),
            "ai".to_string(),
            "analytics".to_string(),
        ],
        url: Some("https://www.sbir.gov/".to_string()),
    };
    let small_grant = GrantOpportunity {
        grant_id: Some("tiny".to_string()),
        title: "Tiny archive grant".to_string(),
        agency: Some("Library".to_string()),
        program: None,
        amount: Some(1_000.0),
        due_date: None,
        eligibility: None,
        topics: vec!["archives".to_string()],
        url: None,
    };
    let records = vec![
        test_record(
            "grant-1",
            "sbir",
            "sbir",
            &["energy", "ai", "grant"],
            &[("awardAmountUsd", 250_000.0)],
            Some(strong_grant),
        ),
        test_record(
            "grant-2",
            "libraries",
            "state-libraries",
            &["archives", "grant"],
            &[("awardAmountUsd", 1_000.0)],
            Some(small_grant),
        ),
    ];
    let request = GrantMatchRequest {
        request_id: Some("match".to_string()),
        applicant_profile: "small business building AI public data models".to_string(),
        focus_areas: vec!["energy".to_string(), "ai".to_string()],
        dataset_ids: None,
        min_amount: Some(50_000.0),
        limit: Some(5),
    };
    let matches = grant_matches_from_records(&records, &request);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].record_id, "grant-1");
    assert!(matches[0].score > 0.0);
    assert!(matches[0]
        .reasons
        .iter()
        .any(|reason| reason.contains("focus-area")));
}

#[test]
fn ui_helpers_escape_html_and_build_scrape_requests() {
    // maud auto-escapes every interpolated value, so the old manual
    // html_escape helper is gone; assert the same escaping via a real
    // render path instead.
    let notice = render_ui_notice("SBIR <grant> & \"quotes\"", "detail", false).into_string();
    assert!(notice.contains("SBIR &lt;grant&gt; &amp; &quot;quotes&quot;"));
    assert!(!notice.contains("<grant>"));

    let request = UiScrapeForm {
        source: Some("SBIR".to_string()),
        url: "https://www.sbir.gov/funding/".to_string(),
        dataset_id: Some("sbir-ui".to_string()),
        strategy: Some("cheerio".to_string()),
        selector: Some("main".to_string()),
        tags: Some("grants, energy, grants".to_string()),
        render_javascript: None,
        include_links: Some("on".to_string()),
        pipeline_enabled: Some("on".to_string()),
    }
    .into_scrape_request()
    .expect("ui scrape form should be accepted");

    assert_eq!(request.source, "SBIR");
    assert_eq!(request.dataset_id.as_deref(), Some("sbir-ui"));
    assert_eq!(
        request.tags,
        Some(vec!["grants".to_string(), "energy".to_string()])
    );
    assert_eq!(request.include_links, Some(true));
    assert_eq!(request.render_javascript, Some(false));
    assert!(request.pipeline.is_some());
}

fn test_state() -> AppState {
    AppState {
        config: Arc::new(Config {
            server_auth_secret: Some("secret".to_string()),
            webhook_secret: None,
            allow_unauthenticated: false,
            allow_unauthenticated_webhooks: false,
            scraper_base_url: "http://127.0.0.1:9000".to_string(),
            scraper_auth_secret: None,
            ingest_request_subject: PUBLIC_DATA_INGEST_REQUESTS_SUBJECT.to_string(),
            ingest_result_subject: PUBLIC_DATA_INGEST_RESULTS_SUBJECT.to_string(),
            webhook_event_subject: PUBLIC_DATA_WEBHOOK_EVENTS_SUBJECT.to_string(),
            pipeline_job_subject: PUBLIC_DATA_PIPELINE_JOBS_SUBJECT.to_string(),
            analysis_result_subject: PUBLIC_DATA_ANALYSIS_RESULTS_SUBJECT.to_string(),
            runtime_event_subject: RUNTIME_EVENTS_SUBJECT.to_string(),
            queue_group: PUBLIC_DATA_INGEST_REQUESTS_QUEUE_GROUP.to_string(),
        }),
        metrics: Arc::new(Metrics::default()),
        nats: None,
        http: reqwest::Client::new(),
        store: Arc::new(RwLock::new(PublicDataStore::default())),
    }
}

#[test]
fn root_page_renders_doctype_and_actions() {
    let html = render_root().into_string();
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(html.contains("<title>dd-public-data-server</title>"));
    assert!(html.contains("href=\"./ui\""));
    // CSS embedded verbatim via PreEscaped.
    assert!(html.contains("background: #1f6f70"));
}

#[tokio::test]
async fn ui_shell_renders_htmx_wiring_and_css() {
    let state = test_state();
    let html = render_ui_shell(&state).into_string();
    assert!(html.starts_with("<!DOCTYPE html>"));
    // HTMX script tag and fragment/action wiring preserved verbatim.
    assert!(html.contains("src=\"https://unpkg.com/htmx.org@2.0.4/dist/htmx.min.js\""));
    assert!(html.contains("hx-get=\"./ui/fragments/summary\""));
    assert!(html.contains("hx-trigger=\"load, every 15s\""));
    assert!(html.contains("hx-post=\"./ui/actions/scrape\""));
    assert!(html.contains("hx-target=\"#scrape-result\""));
    assert!(html.contains("hx-swap=\"innerHTML\""));
    assert!(html.contains("hx-disabled-elt=\"button\""));
    assert!(html.contains("hx-trigger=\"load, every 20s\""));
    // CSS embedded verbatim via PreEscaped.
    assert!(html.contains("--teal: #1f6f70"));
}

#[test]
fn scrape_result_wires_refresh_button_and_escapes_dynamic_values() {
    let value = json!({
        "requestId": "req-1",
        "datasetId": "sbir-ui",
        "record": { "title": "Grant <x> & \"y\"" },
        "scraper": { "status": 200 }
    });
    let html = render_ui_scrape_result(&value).into_string();
    assert!(html.contains("hx-get=\"./ui/fragments/summary\""));
    assert!(html.contains("hx-target=\"#summary\""));
    assert!(html.contains("hx-swap=\"innerHTML\""));
    // Dynamic values are auto-escaped and interpolated.
    assert!(html.contains("Grant &lt;x&gt; &amp; &quot;y&quot;"));
    assert!(!html.contains("Grant <x>"));
    assert!(html.contains("scraper status 200."));
}
