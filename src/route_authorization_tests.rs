//! Enumerates the router and pins the unauthenticated surface.
//!
//! This exists because ~440 routes cannot be audited by eye on every change.
//! The tests below discover every `.route(...)` declared by the files that feed
//! [`build_router`] and drive a real request through the composed application
//! for each one, so a route added without auth fails CI rather than shipping.
use super::*;
use axum::body::Body;
use axum::http::Request as HttpRequest;
use tower::ServiceExt;

/// Source files whose `.route(...)` declarations end up in [`build_router`].
///
/// `src/web_server/` is deliberately absent: it builds a different service
/// (`run_web`) with its own router and dependency boundary.
const ROUTED_SOURCES: &[&str] = &[
    "src/lib.rs",
    "src/transport/mod.rs",
    "src/additive_printing/http.rs",
];

/// Extract the path literal of every `.route(` call in `source`.
///
/// Deliberately textual and deliberately dumb: it must keep working when a
/// route is added in either the single-line or the rustfmt-wrapped form,
/// and it must not depend on any registry a new route could forget to join.
fn route_paths(source: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut rest = source;
    while let Some(offset) = rest.find(".route(") {
        rest = &rest[offset + ".route(".len()..];
        let Some(open) = rest.find('"') else { break };
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('"') else {
            break;
        };
        let path = &after_open[..close];
        // A route path always starts with '/'; anything else means the call
        // was `.route(SOME_CONST, ...)` and the quote we found belongs to a
        // later expression.
        if path.starts_with('/') {
            paths.push(path.to_string());
        }
        rest = after_open;
    }
    paths
}

fn declared_routes() -> Vec<String> {
    let mut paths: Vec<String> = ROUTED_SOURCES
        .iter()
        .flat_map(|file| {
            let source = std::fs::read_to_string(file)
                .unwrap_or_else(|error| panic!("read {file}: {error}"));
            route_paths(&source)
        })
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Substitute `:param` segments so the path actually matches at runtime.
fn concrete_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with(':') {
                "enumeration-test"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// A state whose gate is *enabled*, so a request without a bearer token is
/// rejected with 401 rather than the 503 an unconfigured gate returns. That
/// distinction is what makes the assertions below prove the gate is wired
/// up, not merely that the service is misconfigured.
fn gated_state() -> AppState {
    let auth = config::AuthConfig {
        shared_auth_base: "http://shared-auth.test".to_string(),
        issuer: "https://auth.oresoftware.dev".to_string(),
        audience: "oresoftware".to_string(),
        supabase_url: Some("https://proj.supabase.co".to_string()),
        supabase_api_key: Some("enumeration-test-key".to_string()),
        introspect_secret: Some("introspect-test-secret".to_string()),
        provider_tenant: "proj".to_string(),
        allowed_emails: vec!["operator@example.com".to_string()],
        allowed_roles: vec!["daedalus-operator".to_string()],
        arm_timeout_ms: 100,
        deadline_ms: 200,
    };
    assert!(auth.is_enabled(), "test fixture must enable the gate");
    AppState {
        verifier: SharedAuthVerifier::from_config(&auth).map(Arc::new),
        nats: None,
        persistence: Persistence::Disabled,
        realtime: EventHub::new(ServiceSurface::Fabrication, 8),
        request_subject: FABRICATION_REQUESTS_SUBJECT.to_string(),
        queue_group: FABRICATION_REQUESTS_QUEUE_GROUP.to_string(),
        result_subject: FABRICATION_RESULTS_SUBJECT.to_string(),
        event_subject: RUNTIME_EVENTS_SUBJECT.to_string(),
        mdp_subject: MDP_OPTIMIZE_SUBJECT.to_string(),
        mdp_autopublish: false,
        nats_inflight: Arc::new(Semaphore::new(1)),
        coordination: Arc::new(NoopCoordination::default()),
        lease_ttl: Duration::from_millis(coordination::DEFAULT_LEASE_TTL_MS),
        metrics: Arc::new(Metrics::default()),
        jobs: Arc::new(stores::InMemoryJobStore::default()),
        learning: Arc::new(stores::InMemoryLearningStore::default()),
    }
}

fn gated_app() -> Router {
    build_router(gated_state(), EventHub::new(ServiceSurface::Fabrication, 8))
}

async fn status_for(app: &Router, method: &str, path: &str) -> StatusCode {
    let request = HttpRequest::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .expect("build request");
    app.clone()
        .oneshot(request)
        .await
        .expect("router is infallible")
        .status()
}

#[tokio::test]
async fn every_route_outside_the_public_allowlist_is_gated() {
    let routes = declared_routes();
    // Guard against a scanner that silently stops finding routes: the real
    // surface is ~440 paths, so anything near zero means this test has
    // stopped testing anything.
    assert!(
        routes.len() > 300,
        "route enumeration found only {} paths; the scanner is broken",
        routes.len()
    );

    let app = gated_app();
    let mut unguarded = Vec::new();
    for path in &routes {
        if PUBLIC_ROUTES.contains(&path.as_str()) {
            continue;
        }
        // The gate is a `route_layer`, so it runs for a matched path before
        // method routing: GET alone proves the layer is in front of the
        // handler for every method that path serves.
        let status = status_for(&app, "GET", &concrete_path(path)).await;
        if status != StatusCode::UNAUTHORIZED {
            unguarded.push(format!("{path} -> {status}"));
        }
    }
    assert!(
        unguarded.is_empty(),
        "these routes answered an anonymous request instead of 401. Either put them \
             in authenticated_router (the default) or add them to PUBLIC_ROUTES with a \
             written justification:\n{}",
        unguarded.join("\n")
    );
}

#[tokio::test]
async fn the_public_surface_is_exactly_the_allowlist() {
    // The other direction: nothing may quietly *join* the public set. Every
    // path served without a token must be one this list names.
    let app = gated_app();
    let public: Vec<&str> = declared_routes()
        .iter()
        .map(String::as_str)
        .filter(|path| PUBLIC_ROUTES.contains(path))
        .map(|path| PUBLIC_ROUTES.iter().find(|p| *p == &path).copied().unwrap())
        .collect();
    assert_eq!(
        public,
        vec!["/healthz", "/readyz"],
        "the only routes this crate declares without the operator gate are the \
             kubelet probes; /internal/* comes from dd_runtime_config_client and carries \
             its own shared-secret auth"
    );

    assert_eq!(
        status_for(&app, "GET", "/healthz").await,
        StatusCode::OK,
        "liveness must answer without a token or an auth outage looks like a crash loop"
    );
    assert_eq!(status_for(&app, "GET", "/readyz").await, StatusCode::OK);
}

#[tokio::test]
async fn the_policy_poisoning_and_job_surfaces_are_gated() {
    // Named explicitly so a refactor that drops the layer fails with a
    // readable test name, not just a 400-line diff in the enumeration test.
    let app = gated_app();
    for (method, path) in [
        ("POST", "/learning/observe"),
        ("POST", "/fabrication/learning/observe"),
        ("GET", "/learning/policy"),
        ("GET", "/jobs"),
        ("GET", "/jobs/some-job"),
        ("GET", "/jobs/some-job/release-bundle"),
        ("GET", "/jobs/some-job/artifacts/some-artifact"),
        ("GET", "/metrics"),
        ("POST", "/plan"),
    ] {
        assert_eq!(
            status_for(&app, method, path).await,
            StatusCode::UNAUTHORIZED,
            "{method} {path} must require an operator"
        );
    }
}

#[tokio::test]
async fn realtime_surfaces_reject_before_the_websocket_upgrade() {
    // The upgrade must never happen for an anonymous caller: a 401 response
    // means no socket was opened, whereas a 101 would mean the plan stream
    // is already flowing by the time anyone checks.
    let app = gated_app();
    for path in [
        "/api/realtime",
        "/mash",
        "/mash/fragment",
        "/fabrication/mash",
    ] {
        assert_eq!(
            status_for(&app, "GET", path).await,
            StatusCode::UNAUTHORIZED,
            "{path} must not serve the retained event envelope anonymously"
        );
    }

    for path in ["/ws/json", "/ws/html"] {
        let request = HttpRequest::builder()
            .method("GET")
            .uri(path)
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(Body::empty())
            .expect("build upgrade request");
        let response = app
            .clone()
            .oneshot(request)
            .await
            .expect("router is infallible");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{path} upgraded an unauthenticated client"
        );
    }
}

#[tokio::test]
async fn the_control_plane_surface_is_covered_by_the_body_limit() {
    // Regression test: /internal/* used to be merged *after*
    // `.layer(DefaultBodyLimit::max(..))`, so it accepted unbounded bodies.
    let app = gated_app();
    let oversized = vec![b'a'; MAX_HTTP_BODY_BYTES + 1];
    let request = HttpRequest::builder()
        .method("POST")
        .uri(dd_runtime_config_client::APPLY_ROUTE_PATH)
        .header("content-type", "application/json")
        .body(Body::from(oversized))
        .expect("build request");
    let status = app
        .oneshot(request)
        .await
        .expect("router is infallible")
        .status();
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

/// The security headers are applied as the outermost layer, which is the only
/// placement that reaches all three surfaces.
///
/// `Router::layer` only wraps routes added *before* it, so a layer placed before
/// the `/internal/*` merge would skip that surface entirely — the same ordering
/// trap `the_control_plane_surface_is_covered_by_the_body_limit` pins. Error
/// responses are included on purpose: a 401 or 404 body is still sniffable and
/// still framable.
#[tokio::test]
async fn every_surface_carries_the_security_headers() {
    let cases = [
        ("/healthz", "public route"),
        ("/", "authenticated route (401)"),
        (dd_runtime_config_client::SNAPSHOT_ROUTE_PATH, "/internal/*"),
        ("/not-a-route-at-all", "404 fallback"),
    ];
    for (path, description) in cases {
        let response = gated_app()
            .oneshot(
                HttpRequest::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("router is infallible");
        let headers = response.headers();
        for header in [
            "content-security-policy",
            "x-content-type-options",
            "x-frame-options",
            "referrer-policy",
            "cache-control",
            "permissions-policy",
        ] {
            assert!(
                headers.contains_key(header),
                "{description} ({path}) is missing {header}"
            );
        }
        assert_eq!(
            headers["content-security-policy"],
            transport::CSP,
            "{description} ({path}) must carry the shared policy"
        );
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["cache-control"], "private, no-store");
    }
}
