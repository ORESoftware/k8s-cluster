//! HTTP router and middleware composition.

use crate::state::AppState;
use crate::{enrollment, login, metrics, telemetry};
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(login::page_handler))
        .route("/login", get(login::page_handler).post(login::submit))
        .route("/enroll", get(enrollment::page_handler))
        .route("/enroll/verify", post(enrollment::verify))
        .route("/livez", get(live))
        .route("/healthz", get(live))
        .route("/readyz", get(live))
        .route("/metrics", get(metrics::prometheus))
        .layer(telemetry::http_trace_layer())
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state.metrics),
            metrics::record_http,
        ))
        .with_state(state)
}

async fn live() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::totp;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use axum::response::Response;
    use http_body_util::BodyExt;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower::ServiceExt;

    const ENROLL_HEADING: &str = "Scan with your authenticator";
    const ENROLL_SUBTEXT: &str = "Scan the QR code with Authy, Google Authenticator, 1Password, \
                                  or any TOTP app, then enter the 6-digit code it shows.";

    fn test_state() -> AppState {
        AppState::accepting_for_tests(None, b"test-server-secret".to_vec()).expect("test metrics")
    }

    async fn body_string(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn root_and_login_render_login_form() {
        for uri in ["/", "/login"] {
            let response = router(test_state())
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "status for {uri}");
            let body = body_string(response).await;
            assert!(body.contains(r#"hx-post="/login""#), "form post for {uri}");
            assert!(body.contains(r#"name="email""#), "email input for {uri}");
            assert!(
                body.contains(r#"name="password""#),
                "password input for {uri}"
            );
            assert!(
                body.contains("Authentication not configured"),
                "notice for {uri}"
            );
        }
    }

    #[tokio::test]
    async fn login_without_supabase_config_is_503() {
        let response = router(test_state())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("email=sam%40example.com&password=hunter2"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(body_string(response)
            .await
            .contains("Authentication is not configured"));
    }

    #[tokio::test]
    async fn enroll_without_session_redirects_to_login() {
        let response = router(test_state())
            .oneshot(
                Request::builder()
                    .uri("/enroll")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers().get(header::LOCATION).unwrap(), "/login");
    }

    #[tokio::test]
    async fn enroll_with_session_shows_copy_and_qr() {
        let response = router(test_state())
            .oneshot(
                Request::builder()
                    .uri("/enroll")
                    .header(header::COOKIE, "threefa_session=test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("threefa_enroll="));
        let body = body_string(response).await;
        assert!(body.contains(ENROLL_HEADING));
        assert!(body.contains(ENROLL_SUBTEXT));
        assert!(body.contains("<svg"));
    }

    /// Enroll against `state` so tests can keep replay state across requests
    /// (`AppState` is `Clone` over shared `Arc`s).
    async fn enroll_cookie_on(state: &AppState) -> (String, Vec<u8>) {
        let response = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/enroll")
                    .header(header::COOKIE, "threefa_session=test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let value = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .strip_prefix("threefa_enroll=")
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let secret = totp::base32_decode(value.split('.').next().unwrap()).unwrap();
        (value, secret)
    }

    async fn enroll_cookie() -> (String, Vec<u8>) {
        enroll_cookie_on(&test_state()).await
    }

    async fn post_verify_on(state: &AppState, cookie: &str, code: &str) -> (StatusCode, String) {
        let response = router(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/enroll/verify")
                    .header(
                        header::COOKIE,
                        format!("threefa_session=test-token; threefa_enroll={cookie}"),
                    )
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!("code={code}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        (status, body_string(response).await)
    }

    async fn post_verify(cookie: &str, code: &str) -> (StatusCode, String) {
        post_verify_on(&test_state(), cookie, code).await
    }

    #[tokio::test]
    async fn verify_accepts_valid_code() {
        let (cookie, secret) = enroll_cookie().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let (status, body) = post_verify(&cookie, &totp::totp_code(&secret, now)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Device enrolled"));
    }

    #[tokio::test]
    async fn verify_rejects_tampered_cookie() {
        let (cookie, secret) = enroll_cookie().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let tampered = format!("AAAAAAAA{}", &cookie[8..]);
        let (status, _) = post_verify(&tampered, &totp::totp_code(&secret, now)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn verify_rejects_missing_and_malformed_enrollment_cookies() {
        let response = router(test_state())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/enroll/verify")
                    .header(header::COOKIE, "threefa_session=test-token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("code=000000"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(body_string(response)
            .await
            .contains("No enrollment in progress"));

        let (status, body) = post_verify("malformed", "000000").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Enrollment cookie is malformed"));
    }

    #[tokio::test]
    async fn verify_rejects_a_well_formed_but_wrong_code() {
        let (cookie, secret) = enroll_cookie().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let valid = (-totp::SKEW_STEPS..=totp::SKEW_STEPS)
            .filter_map(|offset| (now / totp::STEP_SECONDS).checked_add_signed(offset))
            .map(|counter| totp::hotp(&secret, counter, 6))
            .collect::<Vec<_>>();
        let wrong = (0..=3u32)
            .map(|value| format!("{value:06}"))
            .find(|candidate| !valid.contains(candidate))
            .unwrap();
        let (status, body) = post_verify(&cookie, &wrong).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("didn&#39;t match") || body.contains("didn't match"));
        assert!(!body.contains("Device enrolled"));
    }

    #[tokio::test]
    async fn enrollment_generates_fresh_160_bit_secrets_shown_on_the_page() {
        let state = test_state();
        let (_, first_secret) = enroll_cookie_on(&state).await;
        let (_, second_secret) = enroll_cookie_on(&state).await;

        // RFC 4226 §4 requires at least a 128-bit shared secret and
        // recommends 160 bits; the handler generates 20 random bytes.
        assert_eq!(first_secret.len(), 20);
        assert_eq!(second_secret.len(), 20);
        assert_ne!(first_secret, second_secret, "secrets must not repeat");

        // The page must show the same secret it sealed into the cookie so
        // manual entry enrolls the authenticator the server will verify.
        let response = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/enroll")
                    .header(header::COOKIE, "threefa_session=test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let cookie_secret = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .strip_prefix("threefa_enroll=")
            .unwrap()
            .split('.')
            .next()
            .unwrap()
            .to_owned();
        let body = body_string(response).await;
        assert!(body.contains(&cookie_secret), "page shows the secret");
    }

    #[tokio::test]
    async fn verify_rejects_a_replayed_code_but_accepts_a_later_one() {
        let state = test_state();
        let (cookie, secret) = enroll_cookie_on(&state).await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let counter = now / totp::STEP_SECONDS;
        let current = totp::hotp(&secret, counter, 6);
        let previous = totp::hotp(&secret, counter - 1, 6);
        let next = totp::hotp(&secret, counter + 1, 6);

        let (status, body) = post_verify_on(&state, &cookie, &current).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Device enrolled"));

        // RFC 6238 §5.2: the exact accepted code must not verify twice.
        let (status, body) = post_verify_on(&state, &cookie, &current).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("already used"), "replay must be rejected");
        assert!(!body.contains("Device enrolled"));

        // An earlier in-window code is also at-or-below the accepted counter.
        if previous != current {
            let (_, body) = post_verify_on(&state, &cookie, &previous).await;
            assert!(body.contains("already used"), "stale code must be rejected");
            assert!(!body.contains("Device enrolled"));
        }

        // A genuinely fresh later code still verifies.
        if next != current {
            let (status, body) = post_verify_on(&state, &cookie, &next).await;
            assert_eq!(status, StatusCode::OK);
            assert!(body.contains("Device enrolled"));
        }
    }

    #[tokio::test]
    async fn replay_tracking_is_scoped_per_enrollment_secret() {
        let state = test_state();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let (first_cookie, first_secret) = enroll_cookie_on(&state).await;
        let (_, body) =
            post_verify_on(&state, &first_cookie, &totp::totp_code(&first_secret, now)).await;
        assert!(body.contains("Device enrolled"));

        // A different enrollment (fresh secret) is unaffected by the first
        // enrollment's accepted counter.
        let (second_cookie, second_secret) = enroll_cookie_on(&state).await;
        let (_, body) = post_verify_on(
            &state,
            &second_cookie,
            &totp::totp_code(&second_secret, now),
        )
        .await;
        assert!(body.contains("Device enrolled"));
    }

    #[tokio::test]
    async fn health_and_metrics_are_available() {
        for uri in ["/livez", "/healthz", "/readyz", "/metrics"] {
            let response = router(test_state())
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "status for {uri}");
        }
    }

    #[tokio::test]
    async fn router_preserves_404_and_records_prometheus_metrics() {
        let app = router(test_state());
        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/not-a-route")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let metrics = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(metrics).await;
        assert!(body.contains("threefa_web_http_requests_total"));
        assert!(body.contains("route=\"unmatched\""));
    }

    #[test]
    fn kubernetes_manifest_matches_runtime_routes_and_submodule_path() {
        let manifest = include_str!("../k8s/ec2/dd-threefa-web-server.deployment.yaml");
        for required in [
            "remote/deployments/3fa-web-server-rs",
            "path: /livez",
            "path: /readyz",
            "prometheus.io/path: /metrics",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "DEPLOYMENT_ENVIRONMENT",
            "SHARED_AUTH_BASE_URL",
            // The sanctioned hop. Pinning the direct dd-shared-auth address
            // here is what kept the NetworkPolicy-denied value in CI.
            "http://dd-remote-gateway.default.svc.cluster.local/shared-auth",
        ] {
            assert!(manifest.contains(required), "manifest missing {required}");
        }
        assert!(!manifest.contains("remote/deployments/threefa-web-server-rs"));
        // Regression guard: shared-auth's ingress NetworkPolicy drops traffic
        // that is not from dd-remote-gateway, so a direct dial silently breaks
        // every login.
        assert!(
            !manifest.contains("value: http://dd-shared-auth."),
            "manifest dials shared-auth directly instead of via the gateway"
        );
    }
}
