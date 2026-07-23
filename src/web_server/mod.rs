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
    shared_auth::{authorize_bearer, SharedAuthVerifier},
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
    auth_http: reqwest::Client,
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
        auth_http: reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?,
    };
    let app = app(state, hub).merge(dd_runtime_config_client::router());
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
    let header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let operator = authorize_bearer(state.verifier.as_deref(), &state.auth_http, header).await?;
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
            auth_http: reqwest::Client::new(),
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
