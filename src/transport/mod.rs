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
}
