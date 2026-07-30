//! Shared MASH, WebSocket, TCP, and NATS adapters.

use axum::{extract::Extension, response::Html, routing::get, Json, Router};
use serde_json::{json, Value};

use crate::realtime::{EventEnvelope, EventHub, ServiceSurface, REALTIME_SCHEMA};

mod nats;
mod tcp;
mod views;
mod websocket;

pub(crate) use nats::{spawn_publisher, spawn_relay};
pub(crate) use tcp::{bind as bind_tcp, serve as serve_tcp};
// Re-exported so `lib.rs` can attach the policy without reaching into the view
// module's internals: the markup and the policy that permits it stay together.
pub(crate) use views::CSP;

pub(crate) fn router<S>(hub: EventHub, surface: ServiceSurface) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/mash", get(mash_page))
        .route("/fabrication/mash", get(mash_page))
        .route("/mash/fragment", get(mash_fragment))
        .route("/api/realtime", get(realtime_snapshot))
        .route("/api/transports", get(transport_contract))
        .route("/ws/html", get(websocket::html_socket))
        .route("/ws/json", get(websocket::json_socket))
        .layer(Extension(surface))
        .layer(Extension(hub))
}

pub(crate) fn page(surface: ServiceSurface, event: &EventEnvelope) -> String {
    views::page(surface, event).into_string()
}

async fn mash_page(
    Extension(hub): Extension<EventHub>,
    Extension(surface): Extension<ServiceSurface>,
) -> Html<String> {
    Html(page(surface, &hub.latest()))
}

async fn mash_fragment(Extension(hub): Extension<EventHub>) -> Html<String> {
    Html(views::status_fragment(&hub.latest()).into_string())
}

async fn realtime_snapshot(Extension(hub): Extension<EventHub>) -> Json<EventEnvelope> {
    Json(hub.latest())
}

async fn transport_contract(Extension(surface): Extension<ServiceSurface>) -> Json<Value> {
    Json(json!({
        "schemaVersion": REALTIME_SCHEMA,
        "service": surface.service_name(),
        "http": {
            "page": "/mash",
            "snapshot": "/api/realtime",
        },
        "websockets": {
            "html": "/ws/html",
            "json": "/ws/json",
        },
        "tcp": {"framing": "newline-delimited-json"},
        "nats": {"subjects": "ORESoftware/k8s-libs-and-shared-defs generated definitions"},
        "persistence": {
            "client": "seaorm",
            "runtimeDdl": false,
            "declarativeContract": "remote/libs/pg-defs/schema/databases/dd_fabrication_server/schema.sql",
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
        response::Response,
    };
    use tower::ServiceExt;

    async fn get(app: &Router, path: &str) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("build transport request"),
            )
            .await
            .expect("transport router is infallible")
    }

    async fn body(response: Response) -> String {
        String::from_utf8(
            to_bytes(response.into_body(), 64 * 1024)
                .await
                .expect("read bounded transport response")
                .to_vec(),
        )
        .expect("transport response is UTF-8")
    }

    #[tokio::test]
    async fn transport_contract_keeps_migration_and_database_boundaries_explicit() {
        let Json(contract) = transport_contract(Extension(ServiceSurface::Web)).await;

        assert_eq!(contract["schemaVersion"], REALTIME_SCHEMA);
        assert_eq!(contract["persistence"]["client"], "seaorm");
        assert_eq!(contract["persistence"]["runtimeDdl"], false);
        assert!(contract["persistence"]["declarativeContract"]
            .as_str()
            .is_some_and(|path| path.contains("dd_fabrication_server")));
    }

    #[tokio::test]
    async fn snapshot_and_html_are_views_of_the_same_event() {
        let hub = EventHub::new(ServiceSurface::Fabrication, 8);
        hub.publish_payload(
            "test",
            "printer.progress",
            json!({"layer": 84, "layers": 120}),
        );
        let Json(snapshot) = realtime_snapshot(Extension(hub.clone())).await;
        let Html(fragment) = mash_fragment(Extension(hub)).await;

        assert_eq!(snapshot.kind, "printer.progress");
        assert_eq!(snapshot.payload["layer"], 84);
        assert!(fragment.contains(&snapshot.event_id));
        assert!(fragment.contains("printer.progress"));
    }

    #[tokio::test]
    async fn composed_router_serves_every_shared_http_adapter() {
        let hub = EventHub::new(ServiceSurface::Fabrication, 8);
        hub.publish_payload(
            "refactor-test",
            "printer.layer.completed",
            json!({"jobId": "fab-42", "layer": 17}),
        );
        let app = router::<()>(hub, ServiceSurface::Fabrication);

        for path in ["/mash", "/fabrication/mash", "/mash/fragment"] {
            let response = get(&app, path).await;
            assert_eq!(response.status(), StatusCode::OK, "GET {path}");
            assert_eq!(
                response.headers().get(header::CONTENT_TYPE),
                Some(&header::HeaderValue::from_static(
                    "text/html; charset=utf-8"
                )),
                "GET {path} must remain an HTML adapter"
            );
            let response_body = body(response).await;
            assert!(response_body.contains("printer.layer.completed"));
            assert!(response_body.contains("fab-42"));
        }

        let snapshot = get(&app, "/api/realtime").await;
        assert_eq!(snapshot.status(), StatusCode::OK);
        let snapshot: Value = serde_json::from_str(&body(snapshot).await).expect("snapshot JSON");
        assert_eq!(snapshot["schemaVersion"], REALTIME_SCHEMA);
        assert_eq!(snapshot["kind"], "printer.layer.completed");
        assert_eq!(snapshot["payload"]["layer"], 17);

        let contract = get(&app, "/api/transports").await;
        assert_eq!(contract.status(), StatusCode::OK);
        let contract: Value = serde_json::from_str(&body(contract).await).expect("contract JSON");
        assert_eq!(contract["service"], "dd-fabrication-server");
        assert_eq!(contract["websockets"]["html"], "/ws/html");
        assert_eq!(contract["websockets"]["json"], "/ws/json");

        assert_eq!(
            get(&app, "/not-a-transport").await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn shared_router_keeps_process_identity_outside_the_event_schema() {
        for surface in [ServiceSurface::Fabrication, ServiceSurface::Web] {
            let hub = EventHub::new(surface, 8);
            let app = router::<()>(hub, surface);

            let snapshot = get(&app, "/api/realtime").await;
            let snapshot: Value =
                serde_json::from_str(&body(snapshot).await).expect("connected event JSON");
            assert_eq!(snapshot["schemaVersion"], REALTIME_SCHEMA);
            assert_eq!(snapshot["source"], surface.service_name());
            assert_eq!(snapshot["kind"], "transport.connected");

            let contract = get(&app, "/api/transports").await;
            let contract: Value =
                serde_json::from_str(&body(contract).await).expect("transport contract JSON");
            assert_eq!(contract["schemaVersion"], REALTIME_SCHEMA);
            assert_eq!(contract["service"], surface.service_name());

            let page_response = get(&app, "/mash").await;
            let page_body = body(page_response).await;
            assert!(page_body.contains(surface.title()));
        }
    }
}
