//! Reconnecting WebSocket bridge from the fabrication API/worker into the web service.

use std::{error::Error, time::Duration};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use crate::realtime::{EventEnvelope, EventHub};

pub(crate) fn spawn(url: String, hub: EventHub) {
    tokio::spawn(async move {
        let mut retry = Duration::from_millis(250);
        loop {
            match relay_session(&url, &hub).await {
                Ok(()) => tracing::info!(
                    network.protocol.name = "websocket",
                    server.address = "fabrication-backend",
                    "fabrication backend WebSocket closed"
                ),
                Err(error) => tracing::warn!(
                    network.protocol.name = "websocket",
                    server.address = "fabrication-backend",
                    error = %error,
                    "fabrication backend WebSocket unavailable"
                ),
            }
            tokio::time::sleep(retry).await;
            retry = (retry * 2).min(Duration::from_secs(30));
        }
    });
}

#[tracing::instrument(
    name = "web.backend.websocket",
    skip_all,
    fields(otel.kind = "client", network.protocol.name = "websocket")
)]
async fn relay_session(url: &str, hub: &EventHub) -> Result<(), Box<dyn Error + Send + Sync>> {
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await?;
    while let Some(message) = socket.next().await {
        match message? {
            Message::Text(text) => relay_payload(hub, text.as_bytes()),
            Message::Binary(bytes) => relay_payload(hub, &bytes),
            Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
            Message::Close(_) => return Ok(()),
            Message::Pong(_) | Message::Frame(_) => {}
        }
    }
    Ok(())
}

fn relay_payload(hub: &EventHub, payload: &[u8]) {
    if let Ok(event) = serde_json::from_slice::<EventEnvelope>(payload) {
        hub.publish(event);
        return;
    }
    let data = serde_json::from_slice::<Value>(payload)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(payload).into_owned()));
    hub.publish_payload(
        "dd-fabrication-server",
        "backend.message",
        json!({"transport": "websocket", "data": data}),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::realtime::ServiceSurface;

    #[test]
    fn bridge_preserves_versioned_backend_envelopes_without_double_wrapping() {
        let hub = EventHub::new(ServiceSurface::Web, 8);
        let source = EventEnvelope::new(
            "dd-fabrication-server",
            "printer.progress",
            json!({"layer": 7}),
        );
        let encoded = serde_json::to_vec(&source).expect("encode event");

        relay_payload(&hub, &encoded);

        assert_eq!(hub.latest(), source);
    }

    #[test]
    fn bridge_wraps_legacy_json_as_a_compatible_event() {
        let hub = EventHub::new(ServiceSurface::Web, 8);
        relay_payload(&hub, br#"{"legacy":true}"#);

        assert_eq!(hub.latest().kind, "backend.message");
        assert_eq!(hub.latest().payload["data"]["legacy"], true);
    }
}
