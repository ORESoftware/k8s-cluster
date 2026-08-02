use std::sync::{Arc, Mutex};

use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Router;
use base64::Engine;

use super::*;

#[derive(Clone, Debug)]
struct CapturedRequest {
    path: String,
    headers: HeaderMap,
    body: String,
}

#[derive(Clone)]
struct MockState {
    status: StatusCode,
    response_body: &'static str,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
}

async fn mock_handler(State(state): State<MockState>, request: Request) -> (StatusCode, String) {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 64 * 1024).await.unwrap();
    state.requests.lock().unwrap().push(CapturedRequest {
        path: parts.uri.path().to_string(),
        headers: parts.headers,
        body: String::from_utf8(body.to_vec()).unwrap(),
    });
    (state.status, state.response_body.to_string())
}

async fn mock_server(
    status: StatusCode,
    response_body: &'static str,
) -> (String, Arc<Mutex<Vec<CapturedRequest>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = MockState {
        status,
        response_body,
        requests: requests.clone(),
    };
    let app = Router::new().fallback(mock_handler).with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), requests)
}

fn config() -> TwilioVerifyConfig {
    TwilioVerifyConfig {
        account_sid: Some("AC00000000000000000000000000000000".into()),
        auth_token: Some("twilio-test-token".into()),
        service_sid: Some("VA11111111111111111111111111111111".into()),
    }
}

fn assert_basic_auth(headers: &HeaderMap) {
    let value = headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .strip_prefix("Basic ")
        .unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .unwrap();
    assert_eq!(
        String::from_utf8(decoded).unwrap(),
        "AC00000000000000000000000000000000:twilio-test-token"
    );
}

#[tokio::test]
async fn twilio_start_contract_uses_basic_auth_and_sms_form() {
    let (base, requests) = mock_server(StatusCode::OK, r#"{"status":"pending"}"#).await;
    start_sms_verification_at(
        &reqwest::Client::new(),
        &config(),
        &base,
        "+14155550100",
    )
    .await
    .unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        request.path,
        "/Services/VA11111111111111111111111111111111/Verifications"
    );
    assert_basic_auth(&request.headers);
    assert!(request.body.contains("To=%2B14155550100"));
    assert!(request.body.contains("Channel=sms"));
}

#[tokio::test]
async fn twilio_check_contract_accepts_only_approved_codes() {
    let (base, requests) = mock_server(StatusCode::OK, r#"{"status":"approved"}"#).await;
    let approved = check_sms_verification_at(
        &reqwest::Client::new(),
        &config(),
        &base,
        "+14155550100",
        "123456",
    )
    .await
    .unwrap();
    assert!(approved);

    let requests = requests.lock().unwrap();
    let request = &requests[0];
    assert_eq!(
        request.path,
        "/Services/VA11111111111111111111111111111111/VerificationCheck"
    );
    assert_basic_auth(&request.headers);
    assert!(request.body.contains("To=%2B14155550100"));
    assert!(request.body.contains("Code=123456"));
}

#[tokio::test]
async fn twilio_unknown_or_expired_verification_is_false() {
    let (base, _) = mock_server(StatusCode::NOT_FOUND, "").await;
    let approved = check_sms_verification_at(
        &reqwest::Client::new(),
        &config(),
        &base,
        "+14155550100",
        "000000",
    )
    .await
    .unwrap();
    assert!(!approved);
}

#[tokio::test]
async fn twilio_provider_error_fails_closed() {
    let (base, _) = mock_server(StatusCode::TOO_MANY_REQUESTS, "rate limited").await;
    let result = start_sms_verification_at(
        &reqwest::Client::new(),
        &config(),
        &base,
        "+14155550100",
    )
    .await;
    assert!(matches!(result, Err(AuthError::Upstream)));
}
