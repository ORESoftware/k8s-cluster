//! End-to-end style tests for the `/webhook/github` route.
//!
//! We send synthetic GitHub deliveries through the real axum router and
//! assert on the HTTP response. The analysis pipeline does spawn a
//! background task on accepted deliveries; that task will fail fast because
//! the synthetic `clone_url` does not resolve, but it has no effect on the
//! response we assert against here.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use formal_methods_service::analysis::pipeline::Pipeline;
use formal_methods_service::config::Config;
use formal_methods_service::dedupe::DeliveryDedupe;
use formal_methods_service::github::GithubClient;
use formal_methods_service::path_filter::PathFilter;
use formal_methods_service::repo_allowlist::RepoAllowlist;
use formal_methods_service::routes;
use formal_methods_service::state::AppState;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::Semaphore;
use tower::ServiceExt;

const SECRET: &str = "super-secret";

#[derive(Default, Clone)]
struct StateOverrides {
    allowed_repos: Option<Vec<String>>,
    path_prefixes: Option<Vec<String>>,
}

fn make_state_with(overrides: StateOverrides) -> AppState {
    let allowed_repos = overrides.allowed_repos.unwrap_or_default();
    let path_prefixes = overrides.path_prefixes.unwrap_or_default();

    let config = Config {
        bind_addr: "0.0.0.0".into(),
        port: 0,
        github_webhook_secret: SECRET.into(),
        github_token: None,
        github_api_base_url: "https://api.github.com".into(),
        workdir_root: PathBuf::from(".work"),
        contract_manifest_path: PathBuf::from("Cargo.toml"),
        cargo_test_package: None,
        cargo_test_features: None,
        max_concurrent_analyses: 1,
        analyzer_timeout: Duration::from_secs(60),
        enable_cargo_check: true,
        enable_cargo_test: true,
        enable_proptest: true,
        enable_kani: true,
        enable_verus: true,
        enable_dreal: true,
        enable_certora: false,
        proptest_test_target: "proptest_props".into(),
        verus_proof_crate_dir: PathBuf::from("proofs/verus"),
        dreal_queries_dir: PathBuf::from("proofs/dreal"),
        dreal_precision: 0.001,
        certora_conf_dir: PathBuf::from("proofs/certora/conf"),
        allowed_repos: allowed_repos.clone(),
        path_prefixes: path_prefixes.clone(),
        delivery_dedupe_capacity: 16,
        delivery_dedupe_ttl: Duration::from_secs(60),
        max_pr_files_pages: 3,
        status_context: "formal-methods/analysis".into(),
    };

    let github = GithubClient::new(config.github_api_base_url.clone(), None).unwrap();
    let pipeline = Pipeline::from_config(&config);

    AppState {
        config: Arc::new(config),
        github: Arc::new(github),
        pipeline: Arc::new(pipeline),
        analysis_semaphore: Arc::new(Semaphore::new(1)),
        repo_allowlist: Arc::new(RepoAllowlist::from_config(&allowed_repos)),
        path_filter: Arc::new(PathFilter::from_config(&path_prefixes)),
        delivery_dedupe: Arc::new(Mutex::new(DeliveryDedupe::new(16, Duration::from_secs(60)))),
    }
}

fn make_state() -> AppState {
    make_state_with(StateOverrides::default())
}

fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

fn pr_payload(action: &str, repo_full_name: &str) -> Vec<u8> {
    serde_json::json!({
        "action": action,
        "number": 7,
        "pull_request": {
            "id": 7, "number": 7, "state": "open", "draft": false, "title": "t",
            "head": { "ref": "feat", "sha": "abc1234", "repo": null },
            "base": { "ref": "staging", "sha": "def5678", "repo": null }
        },
        "repository": {
            "id": 1,
            "name": repo_full_name.split('/').nth(1).unwrap_or("r"),
            "full_name": repo_full_name,
            "private": false,
            "clone_url": "https://example.invalid/o/r.git",
            "owner": { "login": repo_full_name.split('/').next().unwrap_or("o") }
        }
    })
    .to_string()
    .into_bytes()
}

#[tokio::test]
async fn health_endpoint_returns_ok() {
    let app = routes::router(make_state());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn ready_endpoint_lists_analyzers_and_hardening_state() {
    let app = routes::router(make_state_with(StateOverrides {
        allowed_repos: Some(vec!["acme/widgets".into()]),
        path_prefixes: Some(vec!["packages/contract/".into()]),
    }));
    let res = app
        .oneshot(
            Request::builder()
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), 64 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let analyzers = value["analyzers"].as_array().unwrap();
    let names: Vec<&str> = analyzers.iter().map(|v| v.as_str().unwrap()).collect();
    for name in &[
        "cargo-check",
        "cargo-test",
        "proptest",
        "kani",
        "verus",
        "dreal",
        "certora",
    ] {
        assert!(names.contains(name), "missing analyzer {name}");
    }
    assert_eq!(
        value["github_token_configured"],
        serde_json::Value::Bool(false)
    );
    assert_eq!(value["repo_allowlist"]["allow_all"], false);
    assert_eq!(value["path_filter"]["active"], true);
    assert_eq!(value["path_filter"]["prefixes"][0], "packages/contract/");
}

#[tokio::test]
async fn rejects_missing_signature() {
    let app = routes::router(make_state());
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/github")
                .header("x-github-event", "pull_request")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rejects_bad_signature() {
    let app = routes::router(make_state());
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/github")
                .header("x-github-event", "pull_request")
                .header("x-hub-signature-256", "sha256=deadbeef")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ping_event_returns_pong() {
    let body = b"{\"zen\":\"Practicality beats purity.\"}";
    let sig = sign(SECRET, body);
    let app = routes::router(make_state());
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/github")
                .header("x-github-event", "ping")
                .header("x-hub-signature-256", &sig)
                .header("x-github-delivery", "deliv-1")
                .body(Body::from(body.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), 64 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["status"], "pong");
}

#[tokio::test]
async fn unsupported_event_returns_accepted_ignored() {
    let body = b"{}";
    let sig = sign(SECRET, body);
    let app = routes::router(make_state());
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/github")
                .header("x-github-event", "issues")
                .header("x-hub-signature-256", &sig)
                .body(Body::from(body.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let bytes = to_bytes(res.into_body(), 64 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["status"], "ignored");
    assert_eq!(value["reason"], "unsupported_event");
}

#[tokio::test]
async fn closed_pull_request_is_ignored() {
    let body = pr_payload("closed", "o/r");
    let sig = sign(SECRET, &body);
    let app = routes::router(make_state());
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/github")
                .header("x-github-event", "pull_request")
                .header("x-hub-signature-256", &sig)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let bytes = to_bytes(res.into_body(), 64 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["status"], "ignored");
    assert_eq!(value["reason"], "action_not_analyzable");
}

#[tokio::test]
async fn opened_pull_request_is_accepted() {
    let body = pr_payload("opened", "o/r");
    let sig = sign(SECRET, &body);
    let app = routes::router(make_state());
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/github")
                .header("x-github-event", "pull_request")
                .header("x-hub-signature-256", &sig)
                .header("x-github-delivery", "deliv-opened")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let bytes = to_bytes(res.into_body(), 64 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["status"], "accepted");
    assert!(!value["analysis_id"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn rejects_repo_not_in_allowlist() {
    let state = make_state_with(StateOverrides {
        allowed_repos: Some(vec!["acme/widgets".into()]),
        ..StateOverrides::default()
    });
    let body = pr_payload("opened", "attacker/repo");
    let sig = sign(SECRET, &body);
    let app = routes::router(state);
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/github")
                .header("x-github-event", "pull_request")
                .header("x-hub-signature-256", &sig)
                .header("x-github-delivery", "deliv-allowlist")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let bytes = to_bytes(res.into_body(), 64 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["status"], "ignored");
    assert_eq!(value["reason"], "repo_not_allowed");
}

#[tokio::test]
async fn duplicate_delivery_returns_duplicate_status() {
    let state = make_state();
    let app = routes::router(state.clone());
    let body = pr_payload("opened", "o/r");
    let sig = sign(SECRET, &body);

    // First delivery: accepted.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/github")
                .header("x-github-event", "pull_request")
                .header("x-hub-signature-256", &sig)
                .header("x-github-delivery", "deliv-dup")
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let bytes = to_bytes(res.into_body(), 64 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["status"], "accepted");

    // Second delivery with the same id: duplicate.
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/github")
                .header("x-github-event", "pull_request")
                .header("x-hub-signature-256", &sig)
                .header("x-github-delivery", "deliv-dup")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let bytes = to_bytes(res.into_body(), 64 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["status"], "duplicate");
}
