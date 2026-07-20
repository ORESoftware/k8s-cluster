//! Optional server-side Supabase Realtime bridge.
//!
//! Database queries still use SeaORM. This module implements only Supabase's
//! documented WebSocket protocol and relays change notifications to the shared hub.

use std::{error::Error, time::Duration};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use crate::realtime::EventHub;

use super::config::SupabaseConfig;

pub(crate) fn spawn(config: Option<SupabaseConfig>, hub: EventHub) {
    let Some(config) = config else {
        tracing::info!(
            server.address = "supabase-realtime",
            network.protocol.name = "websocket",
            "Supabase Realtime bridge disabled"
        );
        return;
    };
    tokio::spawn(async move {
        let mut retry = Duration::from_millis(500);
        loop {
            match relay_session(&config, &hub).await {
                Ok(()) => tracing::info!(
                    server.address = "supabase-realtime",
                    "Supabase Realtime WebSocket closed"
                ),
                Err(error) => tracing::warn!(
                    server.address = "supabase-realtime",
                    error = %error,
                    "Supabase Realtime WebSocket unavailable"
                ),
            }
            tokio::time::sleep(retry).await;
            retry = (retry * 2).min(Duration::from_secs(30));
        }
    });
}

#[tracing::instrument(
    name = "web.supabase.realtime",
    skip_all,
    fields(otel.kind = "client", network.protocol.name = "websocket", server.address = "supabase-realtime")
)]
async fn relay_session(
    config: &SupabaseConfig,
    hub: &EventHub,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let url = realtime_url(&config.project_url, &config.publishable_key)?;
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await?;
    let topic = format!("realtime:{}", config.topic);
    socket
        .send(Message::Text(join_message(config, &topic).to_string()))
        .await?;

    let mut heartbeat = tokio::time::interval(Duration::from_secs(25));
    let mut reference = 2_u64;
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                let message = json!({
                    "topic": "phoenix",
                    "event": "heartbeat",
                    "payload": {},
                    "ref": reference.to_string(),
                    "join_ref": Value::Null,
                });
                reference += 1;
                socket.send(Message::Text(message.to_string())).await?;
            }
            message = socket.next() => match message {
                Some(Ok(Message::Text(text))) => relay_payload(hub, text.as_bytes()),
                Some(Ok(Message::Binary(bytes))) => relay_payload(hub, &bytes),
                Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await?,
                Some(Ok(Message::Close(_))) | None => return Ok(()),
                Some(Ok(Message::Pong(_) | Message::Frame(_))) => {}
                Some(Err(error)) => return Err(error.into()),
            }
        }
    }
}

fn realtime_url(project_url: &str, publishable_key: &str) -> Result<String, &'static str> {
    let project_url = project_url.trim().trim_end_matches('/');
    let websocket_base = if let Some(host) = project_url.strip_prefix("https://") {
        format!("wss://{host}")
    } else if let Some(host) = project_url.strip_prefix("http://") {
        format!("ws://{host}")
    } else if project_url.starts_with("wss://") || project_url.starts_with("ws://") {
        project_url.to_string()
    } else {
        return Err("SUPABASE_URL must use https, http, wss, or ws");
    };
    if publishable_key.is_empty()
        || !publishable_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("Supabase publishable key contains unsupported URL characters");
    }
    Ok(format!(
        "{websocket_base}/realtime/v1/websocket?apikey={publishable_key}&vsn=1.0.0"
    ))
}

fn join_message(config: &SupabaseConfig, topic: &str) -> Value {
    json!({
        "topic": topic,
        "event": "phx_join",
        "payload": {
            "config": {
                "broadcast": {"ack": false, "self": false},
                "presence": {"enabled": false},
                "postgres_changes": [{
                    "event": "*",
                    "schema": config.schema,
                    "table": config.table,
                }],
                "private": false,
            }
        },
        "ref": "1",
        "join_ref": "1",
    })
}

fn relay_payload(hub: &EventHub, payload: &[u8]) {
    let Ok(message) = serde_json::from_slice::<Value>(payload) else {
        return;
    };
    let event = message
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or("message");
    if matches!(event, "heartbeat" | "phx_reply") {
        return;
    }
    hub.publish_payload(
        "supabase-realtime",
        format!("supabase.{event}"),
        json!({
            "transport": "websocket",
            "topic": message.get("topic").cloned().unwrap_or(Value::Null),
            "data": message.get("payload").cloned().unwrap_or(Value::Null),
        }),
    );
}

#[cfg(test)]
mod tests {
    use crate::realtime::ServiceSurface;

    use super::*;

    fn config() -> SupabaseConfig {
        SupabaseConfig {
            project_url: "https://project.supabase.co/".to_string(),
            publishable_key: "sb_publishable_test-key".to_string(),
            topic: "daedalus-fabrication".to_string(),
            schema: "public".to_string(),
            table: "fabrication_events".to_string(),
        }
    }

    #[test]
    fn realtime_url_uses_websocket_tls_and_only_the_publishable_key() {
        let config = config();
        let url = realtime_url(&config.project_url, &config.publishable_key).expect("URL");

        assert_eq!(
            url,
            "wss://project.supabase.co/realtime/v1/websocket?apikey=sb_publishable_test-key&vsn=1.0.0"
        );
        assert!(!url.contains("service_role"));
    }

    #[test]
    fn join_subscribes_to_the_declarative_fabrication_table() {
        let config = config();
        let join = join_message(&config, "realtime:daedalus-fabrication");

        assert_eq!(join["event"], "phx_join");
        assert_eq!(
            join["payload"]["config"]["postgres_changes"][0]["table"],
            "fabrication_events"
        );
        assert_eq!(
            join["payload"]["config"]["postgres_changes"][0]["event"],
            "*"
        );
    }

    #[test]
    fn postgres_change_is_relayed_without_control_frames() {
        let hub = EventHub::new(ServiceSurface::Web, 8);
        relay_payload(
            &hub,
            br#"{"topic":"realtime:daedalus-fabrication","event":"postgres_changes","payload":{"data":{"record":{"status":"printing"}}}}"#,
        );

        assert_eq!(hub.latest().kind, "supabase.postgres_changes");
        assert_eq!(
            hub.latest().payload["data"]["data"]["record"]["status"],
            "printing"
        );
    }
}
