//! Transport-neutral realtime messages shared by the fabrication and web services.

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;

pub(crate) const REALTIME_SCHEMA: &str = "dd.fabrication.realtime.v1";

static NEXT_EVENT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceSurface {
    Fabrication,
    Web,
}

impl ServiceSurface {
    pub(crate) const fn service_name(self) -> &'static str {
        match self {
            Self::Fabrication => "dd-fabrication-server",
            Self::Web => "dd-fabrication-web-server",
        }
    }

    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Fabrication => "Fabrication API / worker",
            Self::Web => "Fabrication web server",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventEnvelope {
    pub(crate) schema_version: String,
    pub(crate) event_id: String,
    pub(crate) source: String,
    pub(crate) kind: String,
    pub(crate) occurred_at_unix_ms: u128,
    pub(crate) payload: Value,
}

impl EventEnvelope {
    pub(crate) fn new(source: impl Into<String>, kind: impl Into<String>, payload: Value) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let sequence = NEXT_EVENT_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            schema_version: REALTIME_SCHEMA.to_string(),
            event_id: format!("realtime-{timestamp}-{sequence}"),
            source: source.into(),
            kind: kind.into(),
            occurred_at_unix_ms: timestamp,
            payload,
        }
    }

    pub(crate) fn connected(surface: ServiceSurface) -> Self {
        Self::new(
            surface.service_name(),
            "transport.connected",
            json!({
                "service": surface.service_name(),
                "surface": surface.title(),
                "transports": ["http", "websocket-html", "websocket-json", "tcp", "nats"],
                "databaseClient": "seaorm",
            }),
        )
    }
}

#[derive(Clone)]
pub(crate) struct EventHub {
    sender: broadcast::Sender<EventEnvelope>,
    latest: Arc<RwLock<EventEnvelope>>,
    published_total: Arc<AtomicU64>,
}

impl EventHub {
    pub(crate) fn new(surface: ServiceSurface, capacity: usize) -> Self {
        let initial = EventEnvelope::connected(surface);
        let (sender, _) = broadcast::channel(capacity.max(8));
        Self {
            sender,
            latest: Arc::new(RwLock::new(initial)),
            published_total: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn latest(&self) -> EventEnvelope {
        self.latest
            .read()
            .map(|event| event.clone())
            .unwrap_or_else(|_| {
                EventEnvelope::new(
                    "dd-fabrication-realtime",
                    "transport.state-unavailable",
                    json!({"reason": "event hub read lock poisoned"}),
                )
            })
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.sender.subscribe()
    }

    pub(crate) fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    pub(crate) fn published_total(&self) -> u64 {
        self.published_total.load(Ordering::Relaxed)
    }

    pub(crate) fn publish(&self, event: EventEnvelope) -> usize {
        self.published_total.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut latest) = self.latest.write() {
            *latest = event.clone();
        }
        self.sender.send(event).unwrap_or_default()
    }

    pub(crate) fn publish_payload(
        &self,
        source: impl Into<String>,
        kind: impl Into<String>,
        payload: Value,
    ) -> usize {
        self.publish(EventEnvelope::new(source, kind, payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trip_preserves_unknown_payload_fields() {
        let envelope = EventEnvelope::new(
            "migration-test",
            "fabrication.job.updated",
            json!({"status": "queued", "futureField": {"nested": true}}),
        );
        let encoded = serde_json::to_string(&envelope).expect("serialize envelope");
        let decoded: EventEnvelope = serde_json::from_str(&encoded).expect("deserialize envelope");

        assert_eq!(decoded.schema_version, REALTIME_SCHEMA);
        assert_eq!(decoded.kind, "fabrication.job.updated");
        assert_eq!(decoded.payload["futureField"]["nested"], true);
    }

    #[tokio::test]
    async fn hub_fans_out_one_envelope_across_transport_adapters() {
        let hub = EventHub::new(ServiceSurface::Fabrication, 2);
        let mut html_adapter = hub.subscribe();
        let mut json_adapter = hub.subscribe();
        let event = EventEnvelope::new("test", "job.completed", json!({"jobId": "fab-1"}));

        assert_eq!(hub.publish(event.clone()), 2);
        assert_eq!(html_adapter.recv().await.expect("html event"), event);
        assert_eq!(json_adapter.recv().await.expect("json event"), event);
        assert_eq!(hub.latest(), event);
        assert_eq!(hub.published_total(), 1);
        assert_eq!(hub.subscriber_count(), 2);
    }

    #[test]
    fn both_service_surfaces_publish_the_same_versioned_contract() {
        for surface in [ServiceSurface::Fabrication, ServiceSurface::Web] {
            let event = EventEnvelope::connected(surface);
            assert_eq!(event.schema_version, REALTIME_SCHEMA);
            assert_eq!(event.payload["databaseClient"], "seaorm");
            assert_eq!(event.payload["transports"][4], "nats");
        }
    }
}
