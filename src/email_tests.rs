use std::sync::{Arc, Mutex};

use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Router;
use serde_json::Value;

use super::*;

#[derive(Clone, Debug)]
struct CapturedRequest {
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

#[derive(Clone)]
struct MockState {
    status: StatusCode,
    response_body: &'static str,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
}

async fn mock_handler(State(state): State<MockState>, request: Request) -> (StatusCode, String) {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 64 * 1024).await.unwrap().to_vec();
    state.requests.lock().unwrap().push(CapturedRequest {
        path: parts.uri.path().to_string(),
        headers: parts.headers,
        body,
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), requests)
}

fn config() -> MagicLinkConfig {
    MagicLinkConfig {
        sendgrid_api_key: Some("secret".into()),
        otp_pepper: Some("test-pepper-at-least-thirty-two-bytes".into()),
        from_email: Some("auth@example.com".into()),
        from_name: "Example".into(),
        link_base_url: Some("https://app.example/auth/magic-link".into()),
        ttl_secs: 900,
        allow_signup: true,
    }
}

#[test]
fn link_contains_only_the_one_time_token_query() {
    let link = magic_link_url(&config(), "sat_magic_a-b_C").unwrap();
    assert_eq!(
        link,
        "https://app.example/auth/magic-link?token=sat_magic_a-b_C"
    );
}

#[test]
fn html_escaping_protects_the_anchor_attribute() {
    assert_eq!(
        escape_html("https://example.test/?a=1&b=\"x\""),
        "https://example.test/?a=1&amp;b=&quot;x&quot;"
    );
}

#[tokio::test]
async fn sendgrid_request_contract_is_bearer_authenticated_and_complete() {
    let (base, requests) = mock_server(StatusCode::ACCEPTED, "").await;
    send_magic_link_to(
        &reqwest::Client::new(),
        &config(),
        &format!("{base}/v3/mail/send"),
        "person@example.com",
        "sat_magic_contract",
        "123456",
    )
    .await
    .unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.path, "/v3/mail/send");
    assert_eq!(
        request
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap(),
        "Bearer secret"
    );

    let payload: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(
        payload["personalizations"][0]["to"][0]["email"].as_str(),
        Some("person@example.com")
    );
    assert_eq!(payload["from"]["email"].as_str(), Some("auth@example.com"));
    let text = payload["content"][0]["value"].as_str().unwrap();
    let html = payload["content"][1]["value"].as_str().unwrap();
    assert!(text.contains("123456"));
    assert!(text.contains("sat_magic_contract"));
    assert!(html.contains("123456"));
    assert!(html.contains("sat_magic_contract"));
}

#[tokio::test]
async fn sendgrid_non_accepted_response_fails_closed() {
    let (base, _) = mock_server(StatusCode::TOO_MANY_REQUESTS, "rate limited").await;
    let result = send_magic_link_to(
        &reqwest::Client::new(),
        &config(),
        &format!("{base}/v3/mail/send"),
        "person@example.com",
        "sat_magic_contract",
        "123456",
    )
    .await;
    assert!(matches!(result, Err(AuthError::Upstream)));
}

#[tokio::test]
async fn missing_sendgrid_delivery_fields_fail_before_network() {
    let mut missing_api_key = config();
    missing_api_key.sendgrid_api_key = None;
    let mut missing_sender = config();
    missing_sender.from_email = None;
    let mut missing_link_base = config();
    missing_link_base.link_base_url = None;

    for (field, incomplete) in [
        ("api key", missing_api_key),
        ("sender", missing_sender),
        ("link base", missing_link_base),
    ] {
        let (base, requests) = mock_server(StatusCode::ACCEPTED, "").await;
        let result = send_magic_link_to(
            &reqwest::Client::new(),
            &incomplete,
            &format!("{base}/v3/mail/send"),
            "person@example.com",
            "sat_magic_contract",
            "123456",
        )
        .await;
        assert!(
            matches!(result, Err(AuthError::Unavailable)),
            "missing {field} must disable delivery"
        );
        assert_eq!(
            requests.lock().unwrap().len(),
            0,
            "missing {field} must fail before contacting SendGrid"
        );
    }
}

#[tokio::test]
async fn malformed_magic_link_base_fails_before_network() {
    let mut invalid = config();
    invalid.link_base_url = Some("not an absolute URL".into());
    let (base, requests) = mock_server(StatusCode::ACCEPTED, "").await;

    let result = send_magic_link_to(
        &reqwest::Client::new(),
        &invalid,
        &format!("{base}/v3/mail/send"),
        "person@example.com",
        "sat_magic_contract",
        "123456",
    )
    .await;

    assert!(matches!(result, Err(AuthError::Internal)));
    assert_eq!(requests.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn sendgrid_payload_encodes_untrusted_token_and_never_embeds_api_key() {
    let raw_token = "\"><script>alert(1)</script>&next=value";
    let (base, requests) = mock_server(StatusCode::ACCEPTED, "").await;
    send_magic_link_to(
        &reqwest::Client::new(),
        &config(),
        &format!("{base}/v3/mail/send"),
        "person@example.com",
        raw_token,
        "123456",
    )
    .await
    .unwrap();

    let requests = requests.lock().unwrap();
    let request = &requests[0];
    let serialized = String::from_utf8(request.body.clone()).unwrap();
    assert!(!serialized.contains("Bearer secret"));
    assert!(!serialized.contains("<script>"));
    assert!(!serialized.contains(raw_token));

    let payload: Value = serde_json::from_slice(&request.body).unwrap();
    for index in [0, 1] {
        let content = payload["content"][index]["value"].as_str().unwrap();
        assert!(!content.contains("<script>"));
        assert!(!content.contains(raw_token));
        assert!(content.contains("token="));
    }
}
