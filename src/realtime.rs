//! Transport-neutral realtime messages shared by the fabrication and web services.

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock, RwLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use uuid::Uuid;

pub(crate) const REALTIME_SCHEMA: &str = "dd.fabrication.realtime.v1";

/// Per-process counter. On its own this is *not* a unique event id: two
/// replicas both start it at 1 and both emit `1, 2, 3, …`, so a consumer that
/// keys on the id would silently conflate two different events. It is unique
/// only in combination with [`process_event_id`].
static NEXT_EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// A random per-process suffix that makes event ids globally unique.
///
/// Deliberately generated here rather than reusing the coordination holder id:
/// that value participates in lock-cancellation authority on fiducia-node and
/// has no business being published on a realtime event that leaves the process.
fn process_event_id() -> &'static str {
    static PROCESS_EVENT_ID: OnceLock<String> = OnceLock::new();
    PROCESS_EVENT_ID.get_or_init(|| Uuid::new_v4().simple().to_string())
}

/// Build a globally unique, roughly sortable event id.
///
/// Layout is `realtime-<unix-ms, 13-wide>-<sequence, 12-wide>-<process>`:
/// the fixed-width numeric prefixes mean lexicographic order matches time
/// order (and, within a millisecond and a process, emission order), while the
/// random suffix is what actually guarantees uniqueness across replicas.
fn event_id(timestamp: u128, sequence: u64) -> String {
    format!(
        "realtime-{timestamp:013}-{sequence:012}-{}",
        process_event_id()
    )
}

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
        let sequence = NEXT_EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Self {
            schema_version: REALTIME_SCHEMA.to_string(),
            event_id: event_id(timestamp, sequence),
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
    fn event_ids_are_unique_across_processes_not_only_within_one() {
        // The per-process counter alone collides across replicas: both pods
        // emit sequence 1, 2, 3… at the same millisecond under the same load.
        // Two ids built from the *same* timestamp and sequence must therefore
        // still differ once the process suffix differs.
        let mine = event_id(1_700_000_000_000, 1);
        let theirs = format!(
            "realtime-{:013}-{:012}-{}",
            1_700_000_000_000_u128, 1, "other"
        );
        assert_ne!(mine, theirs);
        assert!(mine.ends_with(process_event_id()));
        // The suffix is stable for the life of the process, so ids from one
        // replica share a recognizable origin.
        assert_eq!(process_event_id(), process_event_id());

        // Distinct within the process, too.
        let first = EventEnvelope::new("test", "a", json!({}));
        let second = EventEnvelope::new("test", "b", json!({}));
        assert_ne!(first.event_id, second.event_id);
    }

    #[test]
    fn event_ids_sort_by_time_then_emission_order() {
        // Fixed-width numeric fields keep lexicographic order aligned with
        // chronological order, which is what the UI list and any log-scrape
        // ordering rely on.
        let mut ids = vec![
            event_id(1_700_000_000_000, 12),
            event_id(999_999_999_999, 1),
            event_id(1_700_000_000_000, 2),
            event_id(1_700_000_000_001, 1),
        ];
        ids.sort();
        assert_eq!(
            ids,
            vec![
                event_id(999_999_999_999, 1),
                event_id(1_700_000_000_000, 2),
                event_id(1_700_000_000_000, 12),
                event_id(1_700_000_000_001, 1),
            ]
        );
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
