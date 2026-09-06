//! Integration tests for the t2v-web dashboard over an in-memory SQLite DB.
//!
//! These drive the router directly (no socket, no t2v-api) and assert the
//! security posture and MASH wiring that the browser e2e also checks — so the
//! invariants are pinned even where a browser isn't available (e.g. `cargo
//! test` on a machine without Chromium).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot

async fn body_string(
    resp: axum::response::Response,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8_lossy(&bytes).to_string())
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn dashboard_renders_with_stat_cards_and_no_cdn() {
    let app = t2v_web::testkit::build_test_app().await;
    let (status, _h, body) = body_string(app.oneshot(get("/")).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    // MASH dashboard content.
    assert!(body.contains("Voice"), "dashboard hero missing");
    assert!(body.contains("transcriptions"), "stat cards missing");
    // htmx is vendored & same-origin — never a CDN.
    assert!(
        !body.contains("unpkg.com"),
        "dashboard must not reference a CDN"
    );
    assert!(
        body.contains("/assets/htmx.min.js"),
        "must load vendored htmx"
    );
    assert!(
        body.contains("/assets/app.css"),
        "must load self-hosted CSS"
    );
    // Live-stats websocket wiring.
    assert!(
        body.contains("ws-connect=\"/ws/stats\""),
        "ws live-stats wiring missing"
    );
}

#[tokio::test]
async fn every_response_carries_hardening_headers() {
    let app = t2v_web::testkit::build_test_app().await;
    let (status, h, _b) = body_string(app.oneshot(get("/")).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);

    let csp = h.get("content-security-policy").unwrap().to_str().unwrap();
    assert!(csp.contains("default-src 'self'"), "CSP not strict: {csp}");
    assert!(
        csp.contains("script-src 'self'"),
        "script-src must be self-only: {csp}"
    );
    assert!(!csp.contains("unpkg"), "CSP must not allow a CDN: {csp}");

    assert_eq!(h.get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(h.get("x-frame-options").unwrap(), "DENY");
    assert_eq!(h.get("referrer-policy").unwrap(), "no-referrer");
    assert!(h.get("permissions-policy").is_some());
}

#[tokio::test]
async fn vendored_htmx_and_css_are_served_selfhost() {
    let app = t2v_web::testkit::build_test_app().await;

    let (status, h, body) = body_string(
        app.clone()
            .oneshot(get("/assets/htmx.min.js"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(h
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("javascript"));
    // Real htmx payload (UMD factory + version marker), not an empty stub.
    assert!(
        body.contains("htmx") && body.len() > 10_000,
        "htmx payload looks wrong ({} bytes)",
        body.len()
    );

    let (status, h, _b) = body_string(
        app.clone()
            .oneshot(get("/assets/htmx-ws.js"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(h
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("javascript"));

    let (status, h, css) = body_string(app.oneshot(get("/assets/app.css")).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(h
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("text/css"));
    assert!(
        css.contains(".card"),
        "app.css should carry the dashboard styles"
    );
}

#[tokio::test]
async fn healthz_ready() {
    let app = t2v_web::testkit::build_test_app().await;
    let (status, _h, body) = body_string(app.oneshot(get("/healthz")).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.trim(), "ok");
}

#[tokio::test]
async fn readyz_ok_against_migrated_db() {
    let app = t2v_web::testkit::build_test_app().await;
    let (status, _h, body) = body_string(app.oneshot(get("/readyz")).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.trim(), "ready");
}

#[tokio::test]
async fn translate_and_speak_pages_render_forms() {
    let app = t2v_web::testkit::build_test_app().await;

    let (status, _h, body) =
        body_string(app.clone().oneshot(get("/translate")).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("hx-post=\"/translate\""),
        "translate form missing htmx post"
    );
    assert!(
        body.contains("target_lang"),
        "translate form missing target_lang field"
    );

    let (status, _h, body) = body_string(app.oneshot(get("/speak")).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("hx-post=\"/speak\""),
        "tts form missing htmx post"
    );
}

#[tokio::test]
async fn history_page_renders_empty_state() {
    let app = t2v_web::testkit::build_test_app().await;
    let (status, _h, body) = body_string(app.oneshot(get("/history")).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("History"));
    // No rows yet, but the page must render its empty state, not error.
    assert!(body.contains("No translations yet") || body.contains("Recent translations"));
}
