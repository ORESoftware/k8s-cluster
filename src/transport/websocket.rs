//! HTML and JSON WebSocket adapters over one transport-neutral event hub.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Extension,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;

use crate::realtime::{EventEnvelope, EventHub, ServiceSurface};

use super::views;

#[tracing::instrument(
    name = "http.websocket.upgrade",
    skip_all,
    fields(otel.kind = "server", network.protocol.name = "websocket", daedalus.frame.kind = "html")
)]
pub(super) async fn html_socket(
    upgrade: WebSocketUpgrade,
    Extension(hub): Extension<EventHub>,
    Extension(surface): Extension<ServiceSurface>,
) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| serve(socket, hub, surface, FrameKind::Html))
}

#[tracing::instrument(
    name = "http.websocket.upgrade",
    skip_all,
    fields(otel.kind = "server", network.protocol.name = "websocket", daedalus.frame.kind = "json")
)]
pub(super) async fn json_socket(
    upgrade: WebSocketUpgrade,
    Extension(hub): Extension<EventHub>,
    Extension(surface): Extension<ServiceSurface>,
) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| serve(socket, hub, surface, FrameKind::Json))
}

#[derive(Clone, Copy)]
enum FrameKind {
    Html,
    Json,
}

async fn serve(socket: WebSocket, hub: EventHub, surface: ServiceSurface, frame_kind: FrameKind) {
    let (mut sender, mut receiver) = socket.split();
    let mut events = hub.subscribe();

    if sender
        .send(Message::Text(render(frame_kind, &hub.latest())))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => {
                    if sender.send(Message::Text(render(frame_kind, &event))).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    let event = EventEnvelope::new(
                        surface.service_name(),
                        "transport.lagged",
                        serde_json::json!({"skipped": skipped}),
                    );
                    if sender.send(Message::Text(render(frame_kind, &event))).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = receiver.next() => match incoming {
                Some(Ok(Message::Text(text))) if client_requested_refresh(&text) => {
                    if sender.send(Message::Text(render(frame_kind, &hub.latest()))).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Ping(payload))) => {
                    if sender.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            }
        }
    }
}

fn client_requested_refresh(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("action")
                .and_then(|action| action.as_str())
                .map(str::to_owned)
        })
        .is_some_and(|action| action == "refresh" || action == "ping")
}

fn render(frame_kind: FrameKind, event: &EventEnvelope) -> String {
    match frame_kind {
        FrameKind::Html => views::status_fragment(event).into_string(),
        FrameKind::Json => serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn htmx_refresh_message_is_accepted_without_trusting_headers() {
        assert!(client_requested_refresh(
            r#"{"action":"refresh","HEADERS":{"HX-Request":"true"}}"#
        ));
        assert!(!client_requested_refresh(
            r#"{"action":"publish","payload":"untrusted"}"#
        ));
        assert!(!client_requested_refresh("not-json"));
    }

    #[test]
    fn html_and_json_frames_share_the_exact_event_envelope() {
        let event = EventEnvelope::new("nats", "job.updated", json!({"jobId": "fab-42"}));
        let html = render(FrameKind::Html, &event);
        let json = render(FrameKind::Json, &event);
        let decoded: EventEnvelope = serde_json::from_str(&json).expect("JSON frame");

        assert_eq!(decoded, event);
        assert!(html.contains("hx-swap-oob=\"outerHTML\""));
        assert!(html.contains("fab-42"));
    }
}
