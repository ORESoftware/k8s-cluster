//! End-to-end router smoke tests over an in-memory SQLite database.
//!
//! These exercise the endpoints that do not require an external provider:
//! health, metrics, the FFT `/v1/analyze` path (pure DSP), input validation,
//! and the Vapi webhook auth + assistant-request flow. Provider-backed
//! endpoints (STT/TTS/translate) are covered by the crate's unit tests and
//! would need live API keys to run here.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use t2v_api::testkit;
use tower::ServiceExt; // oneshot

async fn test_state() -> testkit::TestApp {
    testkit::build_test_state().await
}

#[tokio::test]
async fn healthz_ok() {
    let state = test_state().await;
    let resp = state
        .app()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn metrics_exposes_prometheus_text() {
    let state = test_state().await;
    let resp = state
        .app()
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("t2v_api_http_requests_total"));
}

#[tokio::test]
async fn analyze_runs_custom_fft_on_a_wav_upload() {
    use t2v_core::audio::encode_wav_pcm16;
    let sample_rate = 8000u32;
    let n = 8000usize;
    let samples: Vec<f64> = (0..n)
        .map(|i| 0.8 * (2.0 * std::f64::consts::PI * 700.0 * i as f64 / sample_rate as f64).sin())
        .collect();
    let wav = encode_wav_pcm16(&samples, sample_rate);

    let state = test_state().await;
    let resp = state
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/analyze")
                .body(Body::from(wav))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["ok"], true);
    let dominant = v["analysis"]["dominantFreqHz"].as_f64().unwrap();
    assert!((dominant - 700.0).abs() < 3.0, "dominant {dominant}");
}

#[tokio::test]
async fn analyze_decodes_dtmf_digits() {
    // Build a DTMF '5' (770 + 1336 Hz) and confirm the analyzer reports it.
    let sample_rate = 8000u32;
    let n = sample_rate as usize / 8; // 125 ms
    let samples: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 / sample_rate as f64;
            0.5 * (2.0 * std::f64::consts::PI * 770.0 * t).sin()
                + 0.5 * (2.0 * std::f64::consts::PI * 1336.0 * t).sin()
        })
        .collect();
    let wav = t2v_core::audio::encode_wav_pcm16(&samples, sample_rate);

    let state = test_state().await;
    let resp = state
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/analyze")
                .body(Body::from(wav))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["analysis"]["dtmfDigits"], "5");
}

#[tokio::test]
async fn translate_rejects_empty_text() {
    let state = test_state().await;
    let resp = state
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/translate")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"text":"   ","target_lang":"Spanish"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn vapi_webhook_rejects_bad_secret() {
    let state = test_state().await.with_vapi_secret("s3cr3t");
    let resp = state
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/vapi/webhook")
                .header("content-type", "application/json")
                .header("x-vapi-secret", "wrong")
                .body(Body::from(r#"{"message":{"type":"status-update"}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn vapi_assistant_request_returns_translator_assistant() {
    let state = test_state().await.with_vapi_secret("s3cr3t");
    let resp = state
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/vapi/webhook")
                .header("content-type", "application/json")
                .header("x-vapi-secret", "s3cr3t")
                .body(Body::from(r#"{"message":{"type":"assistant-request"}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        v["assistant"]["model"]["tools"][0]["function"]["name"],
        "translate_text"
    );
}
