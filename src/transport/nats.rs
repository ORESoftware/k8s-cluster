//! NATS-to-realtime fan-out using the generated shared subject definitions.

use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::realtime::{EventEnvelope, EventHub, ServiceSurface};

const MAX_RELAY_PAYLOAD_BYTES: usize = 512 * 1024;

pub(crate) fn spawn_relay(
    client: Option<async_nats::Client>,
    subject: String,
    hub: EventHub,
    surface: ServiceSurface,
) {
    let Some(client) = client else {
        return;
    };
    tokio::spawn(async move {
        if let Err(error) = run_relay(client, subject.clone(), hub, surface).await {
            tracing::error!(
                messaging.system = "nats",
                messaging.destination.name = %subject,
                error = %error,
                "realtime NATS relay stopped"
            );
        }
    });
}

pub(crate) fn spawn_publisher(client: Option<async_nats::Client>, subject: String, hub: EventHub) {
    let Some(client) = client else {
        return;
    };
    tokio::spawn(async move {
        let mut events = hub.subscribe();
        loop {
            match events.recv().await {
                Ok(event) => {
                    let Ok(payload) = serde_json::to_vec(&event) else {
                        continue;
                    };
                    if let Err(error) = client.publish(subject.clone(), payload.into()).await {
                        tracing::error!(
                            messaging.system = "nats",
                            messaging.operation.name = "publish",
                            messaging.destination.name = %subject,
                            error = %error,
                            "realtime event publish failed"
                        );
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(
                        messaging.system = "nats",
                        messaging.destination.name = %subject,
                        messaging.batch.message_count = skipped,
                        "realtime NATS publisher lagged"
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

#[tracing::instrument(
    name = "messaging.realtime_relay",
    skip_all,
    fields(otel.kind = "consumer", messaging.system = "nats", messaging.destination.name = %subject)
)]
async fn run_relay(
    client: async_nats::Client,
    subject: String,
    hub: EventHub,
    surface: ServiceSurface,
) -> Result<(), async_nats::SubscribeError> {
    let mut subscriber = client.subscribe(subject.clone()).await?;
    while let Some(message) = subscriber.next().await {
        if message.payload.len() > MAX_RELAY_PAYLOAD_BYTES {
            tracing::warn!(
                messaging.system = "nats",
                messaging.destination.name = %message.subject,
                messaging.message.body.size = message.payload.len(),
                "ignored oversized realtime relay message"
            );
            continue;
        }
        hub.publish(envelope_from_payload(
            surface,
            message.subject.as_str(),
            &message.payload,
        ));
    }
    Ok(())
}

fn envelope_from_payload(surface: ServiceSurface, subject: &str, payload: &[u8]) -> EventEnvelope {
    let decoded = serde_json::from_slice::<Value>(payload)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(payload).into_owned()));
    let kind = decoded
        .get("kind")
        .and_then(Value::as_str)
        .or_else(|| decoded.get("schemaVersion").and_then(Value::as_str))
        .unwrap_or("nats.message")
        .to_string();
    EventEnvelope::new(
        surface.service_name(),
        kind,
        json!({
            "transport": "nats",
            "subject": subject,
            "data": decoded,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_preserves_shared_subject_and_versioned_payload() {
        let event = envelope_from_payload(
            ServiceSurface::Web,
            "dd.remote.fabrication.results",
            br#"{"schemaVersion":"fabrication.plan.v1","jobId":"fab-9"}"#,
        );

        assert_eq!(event.kind, "fabrication.plan.v1");
        assert_eq!(event.payload["subject"], "dd.remote.fabrication.results");
        assert_eq!(event.payload["data"]["jobId"], "fab-9");
    }

    #[test]
    fn relay_wraps_non_json_without_panicking() {
        let event = envelope_from_payload(
            ServiceSurface::Fabrication,
            "dd.remote.fabrication.results",
            b"legacy-result",
        );

        assert_eq!(event.kind, "nats.message");
        assert_eq!(event.payload["data"], "legacy-result");
    }

    #[test]
    fn outbound_event_serialization_keeps_the_shared_schema() {
        let event = EventEnvelope::new(
            "dd-fabrication-server",
            "printer.preflight.completed",
            json!({"releaseReady": true}),
        );
        let encoded = serde_json::to_vec(&event).expect("encode outbound NATS event");
        let decoded: EventEnvelope = serde_json::from_slice(&encoded).expect("decode event");

        assert_eq!(decoded, event);
        assert_eq!(decoded.schema_version, crate::realtime::REALTIME_SCHEMA);
    }
}
