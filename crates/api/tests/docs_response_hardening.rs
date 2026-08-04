use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use t2v_api::testkit;
use tower::ServiceExt;

fn assert_common_headers(response: &axum::response::Response, private: bool) {
    let headers = response.headers();
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(headers[header::PRAGMA], "no-cache");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    let permissions = headers["permissions-policy"].to_str().unwrap();
    assert!(permissions.contains("camera=()"));
    assert!(permissions.contains("microphone=()"));
    assert!(permissions.contains("geolocation=()"));
    if private {
        assert!(headers[header::VARY]
            .to_str()
            .unwrap()
            .split(',')
            .any(|value| value.trim().eq_ignore_ascii_case("authorization")));
    }
}

fn assert_html_headers(response: &axum::response::Response) {
    let headers = response.headers();
    let csp = headers["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(csp.contains("base-uri 'none'"));
    assert!(csp.contains("object-src 'none'"));
    assert_eq!(headers["x-frame-options"], "DENY");
}

#[tokio::test]
async fn internal_docs_fail_closed_with_non_cacheable_operator_responses() {
    let disabled = testkit::build_test_state().await;
    let response = disabled
        .app()
        .oneshot(
            Request::builder()
                .uri("/internal/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_common_headers(&response, true);
    assert!(response.headers().get(header::WWW_AUTHENTICATE).is_none());

    let enabled = testkit::build_test_state()
        .await
        .with_server_auth("op-secret");
    let denied = enabled
        .app()
        .oneshot(
            Request::builder()
                .uri("/internal/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    assert_common_headers(&denied, true);
    assert_eq!(
        denied.headers()[header::WWW_AUTHENTICATE],
        "Bearer realm=\"t2v-operator\""
    );
}

#[tokio::test]
async fn authorized_private_docs_and_history_vary_on_authorization() {
    let state = testkit::build_test_state()
        .await
        .with_server_auth("op-secret");

    for uri in [
        "/internal/openapi.json",
        "/internal/docs/api",
        "/v1/history/translations",
    ] {
        let response = state
            .app()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::AUTHORIZATION, "Bearer op-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        assert_common_headers(&response, true);
        if uri.ends_with("/api") {
            assert_html_headers(&response);
        }
    }
}

#[tokio::test]
async fn docs_head_responses_preserve_security_headers_without_bodies() {
    let state = testkit::build_test_state()
        .await
        .with_server_auth("op-secret");

    for (uri, private, html) in [
        ("/openapi.json", false, false),
        ("/api/docs", false, true),
        ("/internal/openapi.json", true, false),
        ("/internal/docs/api", true, true),
    ] {
        let mut builder = Request::builder().method("HEAD").uri(uri);
        if private {
            builder = builder.header(header::AUTHORIZATION, "Bearer op-secret");
        }
        let response = state
            .app()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        assert_common_headers(&response, private);
        if html {
            assert_html_headers(&response);
        }
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(body.is_empty(), "HEAD {uri} returned a body");
    }
}

#[tokio::test]
async fn unsupported_docs_methods_do_not_reflect_operator_credentials() {
    let state = testkit::build_test_state()
        .await
        .with_server_auth("op-secret");

    for uri in [
        "/openapi.json",
        "/api/docs.json",
        "/api/docs",
        "/docs/api",
        "/internal/openapi.json",
        "/internal/docs/api",
    ] {
        let response = state
            .app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::AUTHORIZATION, "Bearer op-secret")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"token":"op-secret"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED, "{uri}");
        assert_common_headers(&response, uri.starts_with("/internal/"));
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(!String::from_utf8_lossy(&body).contains("op-secret"));
    }
}
