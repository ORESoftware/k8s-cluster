//! Runtime composition for the separately deployable browser-facing service.

use std::{error::Error, sync::Arc, time::Duration};

use axum::{
    extract::{DefaultBodyLimit, Request, State},
    middleware::{self, Next},
    response::{Redirect, Response},
    routing::get,
    Router,
};

use crate::{
    error::ServiceError,
    messaging, observability,
    persistence::Persistence,
    realtime::{EventHub, ServiceSurface},
    secrets::SecretOverlay,
    shared_auth::SharedAuthVerifier,
    transport,
};

mod backend;
mod config;
mod http;
mod supabase;

const MAX_HTTP_BODY_BYTES: usize = 512 * 1024;

#[derive(Clone)]
pub(super) struct WebState {
    pub(super) persistence: Persistence,
    pub(super) realtime: EventHub,
    pub(super) nats_enabled: bool,
    pub(super) supabase_enabled: bool,
    verifier: Option<Arc<SharedAuthVerifier>>,
}

pub(crate) async fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = observability::init_for(ServiceSurface::Web.service_name());
    let config = config::WebConfig::from_env()?;
    let persistence = Persistence::from_web_env().await?;
    let persistence_enabled = persistence.is_enabled();
    let secrets = SecretOverlay::load()
        .await
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let nats = messaging::connect_optional(&secrets, ServiceSurface::Web.service_name()).await;
    let nats_enabled = nats.is_some();
    let supabase_enabled = config.supabase.is_some();
    let auth_configured = config.auth.is_enabled();
    let verifier = SharedAuthVerifier::from_config(&config.auth).map(Arc::new);
    let auth_enabled = auth_configured && verifier.is_some();
    if !auth_enabled {
        tracing::warn!(
            service.name = "dd-fabrication-web-server",
            auth.system = "shared-auth",
            event.name = "auth.configuration.missing",
            "shared-auth is not configured; MASH, API, and websocket routes will return 503"
        );
    }
    let hub = EventHub::new(ServiceSurface::Web, config.event_buffer);

    backend::spawn(config.backend_ws_url.clone(), hub.clone());
    supabase::spawn(config.supabase.clone(), hub.clone());
    transport::spawn_relay(
        nats.clone(),
        config.nats_result_subject.clone(),
        hub.clone(),
        ServiceSurface::Web,
    );
    transport::spawn_publisher(nats, config.nats_event_subject.clone(), hub.clone());

    let tcp_address = config.tcp_address()?;
    let tcp_enabled = config.tcp_enabled;
    if tcp_enabled {
        let tcp_listener = transport::bind_tcp(tcp_address).await?;
        let tcp_hub = hub.clone();
        tokio::spawn(async move {
            if let Err(error) =
                transport::serve_tcp(tcp_listener, tcp_hub, ServiceSurface::Web).await
            {
                tracing::error!(
                    network.transport = "tcp",
                    server.address = %tcp_address,
                    error = %error,
                    "fabrication web TCP server stopped"
                );
            }
        });
    } else {
        tracing::info!(
            network.transport = "tcp",
            server.address = %tcp_address,
            "web realtime TCP transport disabled (set FABRICATION_WEB_TCP_ENABLED=true only on a trusted network)"
        );
    }

    let state = WebState {
        persistence,
        realtime: hub.clone(),
        nats_enabled,
        supabase_enabled,
        verifier,
    };
    let app = app(state, hub);
    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let http_address = config.http_address()?;
    observability::web_server_listening(
        http_address,
        tcp_address,
        tcp_enabled,
        persistence_enabled,
        nats_enabled,
        supabase_enabled,
        auth_enabled,
    );
    let listener = tokio::net::TcpListener::bind(http_address).await?;
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    Ok(())
}

fn app(state: WebState, hub: EventHub) -> Router {
    let protected = Router::new()
        .route("/", get(|| async { Redirect::temporary("/mash") }))
        .merge(transport::router(hub, ServiceSurface::Web))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_operator,
        ));

    Router::new()
        .route("/healthz", get(http::healthz))
        .route("/readyz", get(http::readyz))
        .route("/metrics", get(http::metrics))
        .merge(protected)
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        // The runtime-config client owns its own state, so it can only be merged
        // after `with_state` — which means the body limit above does not reach
        // it. Applying the limit to that router directly is what keeps
        // /internal/* from accepting an unbounded request body. It used to be
        // merged in `run_web`, outside this function and after the layer, so the
        // control-plane surface was unbounded here even though the API binary
        // had already been fixed.
        .merge(dd_runtime_config_client::router().layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES)))
        // Last, so it wraps everything above including /internal/* and the 404
        // fallback. This is the browser-facing binary — it serves /mash, the one
        // HTML surface a browser actually loads — so it needs the policy at
        // least as much as the JSON API does.
        .layer(middleware::from_fn(crate::security_headers))
}

#[tracing::instrument(
    name = "daedalus_web.auth.require_operator",
    skip(state, request, next),
    fields(http.request.method = %request.method(), http.route = %request.uri().path())
)]
async fn require_operator(
    State(state): State<WebState>,
    request: Request,
    next: Next,
) -> Result<Response, ServiceError> {
    let verifier = state.verifier.as_deref().ok_or_else(|| {
        ServiceError::Unavailable(
            "shared-auth is not configured; refusing to serve authenticated routes".to_string(),
        )
    })?;
    let operator = verifier.authorize(request.headers()).await?;
    tracing::debug!(
        auth.subject = %operator.subject,
        auth.email = ?operator.email,
        auth.roles = ?operator.roles,
        auth.authority = ?operator.authority,
        event.name = "auth.authorization.succeeded",
        "fabrication web request authorized"
    );
    let mut request = request;
    request.extensions_mut().insert(operator);
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{header, Method, Request, StatusCode},
        response::Response,
    };
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use crate::shared_auth::Operator;

    fn test_operator() -> Operator {
        Operator {
            subject: "shared-web-operator".to_string(),
            email: Some("operator@example.com".to_string()),
            roles: vec!["daedalus-operator".to_string()],
            authority: shared_auth_lib::Authority::SharedAuth,
        }
    }

    fn test_app(hub: EventHub) -> Router {
        let state = WebState {
            persistence: Persistence::Disabled,
            realtime: hub.clone(),
            nats_enabled: false,
            supabase_enabled: false,
            verifier: Some(Arc::new(SharedAuthVerifier::for_test(test_operator()))),
        };
        app(state, hub)
    }

    async fn request_with_auth(app: &Router, method: Method, path: &str) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .expect("build web request"),
            )
            .await
            .expect("web router is infallible")
    }

    async fn request(app: &Router, method: Method, path: &str) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .expect("build web request"),
            )
            .await
            .expect("web router is infallible")
    }

    async fn body(response: Response) -> String {
        String::from_utf8(
            to_bytes(response.into_body(), 64 * 1024)
                .await
                .expect("read bounded web response")
                .to_vec(),
        )
        .expect("web response is UTF-8")
    }

    #[test]
    fn web_runtime_composes_a_separate_router_from_shared_transports() {
        let hub = EventHub::new(ServiceSurface::Web, 8);
        let _: Router = test_app(hub);
    }

    #[tokio::test]
    async fn web_router_preserves_health_readiness_metrics_and_mash_contracts() {
        let hub = EventHub::new(ServiceSurface::Web, 8);
        hub.publish_payload(
            "refactor-test",
            "printer.progress",
            json!({"jobId": "web-fab-7", "percent": 62}),
        );
        let app = test_app(hub);

        let root = request_with_auth(&app, Method::GET, "/").await;
        assert_eq!(root.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            root.headers().get(header::LOCATION),
            Some(&header::HeaderValue::from_static("/mash"))
        );

        let health = request(&app, Method::GET, "/healthz").await;
        assert_eq!(health.status(), StatusCode::OK);
        let health: Value = serde_json::from_str(&body(health).await).expect("health JSON");
        assert_eq!(health["service"], "dd-fabrication-web-server");
        assert_eq!(health["architecture"], "mash");
        assert_eq!(health["databaseClient"], "seaorm");

        let ready = request(&app, Method::GET, "/readyz").await;
        assert_eq!(ready.status(), StatusCode::OK);
        let ready: Value = serde_json::from_str(&body(ready).await).expect("readiness JSON");
        assert_eq!(ready["database"], "disabled");
        assert_eq!(ready["nats"], "disabled");
        assert_eq!(ready["supabaseRealtime"], "disabled");

        let metrics = request(&app, Method::GET, "/metrics").await;
        assert_eq!(metrics.status(), StatusCode::OK);
        assert_eq!(
            metrics.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static(
                "text/plain; version=0.0.4"
            ))
        );
        let metrics = body(metrics).await;
        assert!(metrics.contains("dd_fabrication_web_up 1"));
        assert!(metrics.contains("dd_fabrication_web_realtime_events_published_total 1"));

        let mash = request_with_auth(&app, Method::GET, "/mash").await;
        assert_eq!(mash.status(), StatusCode::OK);
        let mash = body(mash).await;
        assert!(mash.contains("Fabrication web server"));
        assert!(mash.contains("printer.progress"));
        assert!(mash.contains("web-fab-7"));

        let snapshot = request_with_auth(&app, Method::GET, "/api/realtime").await;
        assert_eq!(snapshot.status(), StatusCode::OK);
        let snapshot: Value = serde_json::from_str(&body(snapshot).await).expect("snapshot JSON");
        assert_eq!(snapshot["kind"], "printer.progress");
        assert_eq!(snapshot["payload"]["percent"], 62);
    }

    #[tokio::test]
    async fn mash_json_and_websockets_are_shared_auth_gated() {
        let app = test_app(EventHub::new(ServiceSurface::Web, 8));
        for path in ["/", "/mash", "/api/realtime", "/ws/html", "/ws/json"] {
            assert_eq!(
                request(&app, Method::GET, path).await.status(),
                StatusCode::UNAUTHORIZED,
                "{path} must reject before rendering or upgrading"
            );
        }
        for path in ["/healthz", "/readyz", "/metrics"] {
            assert_eq!(
                request(&app, Method::GET, path).await.status(),
                StatusCode::OK
            );
        }
    }

    /// This binary is the one a browser actually loads `/mash` from, so the
    /// policy has to reach it — and reach the surfaces that are easy to miss.
    ///
    /// The headers are applied as the outermost layer: `Router::layer` only
    /// wraps routes added before it, so a layer placed before the `/internal/*`
    /// merge would skip that surface, and error responses would go bare. A
    /// sniffed or framed 401/404 body is still an attack surface.
    #[tokio::test]
    async fn every_web_surface_carries_the_security_headers() {
        let app = test_app(EventHub::new(ServiceSurface::Web, 8));

        let cases = [
            ("/healthz", "public route"),
            ("/mash", "authenticated HTML surface"),
            (dd_runtime_config_client::SNAPSHOT_ROUTE_PATH, "/internal/*"),
            ("/not-a-route-at-all", "404 fallback"),
        ];
        for (path, description) in cases {
            let response = request(&app, Method::GET, path).await;
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
            assert_eq!(headers["x-frame-options"], "DENY");
        }
    }

    /// Regression test: `/internal/*` used to be merged in `run_web`, *after*
    /// `app()` had already applied `DefaultBodyLimit`, so the control-plane
    /// surface on this binary accepted unbounded request bodies. The API binary
    /// had the identical bug and was fixed; this one was missed because the
    /// merge happened in a different function.
    #[tokio::test]
    async fn the_control_plane_surface_is_covered_by_the_body_limit() {
        let app = test_app(EventHub::new(ServiceSurface::Web, 8));
        let oversized = vec![b'a'; MAX_HTTP_BODY_BYTES + 1];
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(dd_runtime_config_client::APPLY_ROUTE_PATH)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(oversized))
                    .expect("build oversized request"),
            )
            .await
            .expect("web router is infallible");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn web_router_does_not_absorb_fabrication_domain_routes() {
        let app = test_app(EventHub::new(ServiceSurface::Web, 8));

        for (method, path) in [
            (Method::POST, "/plan"),
            (Method::GET, "/printers/catalog"),
            (Method::POST, "/printing/preflight"),
            (Method::GET, "/jobs"),
        ] {
            assert_eq!(
                request(&app, method.clone(), path).await.status(),
                StatusCode::NOT_FOUND,
                "{method} {path} belongs to the fab API/worker, not the web process"
            );
        }
    }
}
