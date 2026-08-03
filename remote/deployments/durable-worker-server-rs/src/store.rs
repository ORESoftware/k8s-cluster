use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use async_nats::jetstream::{
    self,
    kv::{self, Operation},
    stream::{Config as StreamConfig, RetentionPolicy, StorageType},
};
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::TryStreamExt;
use tokio::sync::{Mutex, RwLock};

use crate::model::DurableEvent;

#[derive(Clone, Debug)]
pub struct StoredValue {
    pub revision: u64,
    pub value: Bytes,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("state changed concurrently")]
    Conflict,
    #[error("state backend error: {0}")]
    Backend(String),
    #[error("state serialization error: {0}")]
    Serialization(String),
}

fn classify_write_error(error: impl std::fmt::Display) -> StoreError {
    let message = error.to_string();
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("wrong last sequence")
        || normalized.contains("wrong last subject sequence")
        || normalized.contains("key exists")
        || normalized.contains("expected") && normalized.contains("revision")
    {
        StoreError::Conflict
    } else {
        StoreError::Backend(message)
    }
}

#[async_trait]
pub trait StateStore: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<StoredValue>, StoreError>;
    async fn create(&self, key: &str, value: Bytes) -> Result<u64, StoreError>;
    async fn update(&self, key: &str, revision: u64, value: Bytes) -> Result<u64, StoreError>;
    async fn put(&self, key: &str, value: Bytes) -> Result<u64, StoreError>;
    async fn keys(&self) -> Result<Vec<String>, StoreError>;
}

#[derive(Default)]
pub struct MemoryStore {
    revision: AtomicU64,
    values: RwLock<BTreeMap<String, StoredValue>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_revision(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::SeqCst) + 1
    }
}

#[async_trait]
impl StateStore for MemoryStore {
    async fn get(&self, key: &str) -> Result<Option<StoredValue>, StoreError> {
        Ok(self.values.read().await.get(key).cloned())
    }

    async fn create(&self, key: &str, value: Bytes) -> Result<u64, StoreError> {
        let mut values = self.values.write().await;
        if values.contains_key(key) {
            return Err(StoreError::Conflict);
        }
        let revision = self.next_revision();
        values.insert(key.to_string(), StoredValue { revision, value });
        Ok(revision)
    }

    async fn update(&self, key: &str, revision: u64, value: Bytes) -> Result<u64, StoreError> {
        let mut values = self.values.write().await;
        let Some(current) = values.get(key) else {
            return Err(StoreError::Conflict);
        };
        if current.revision != revision {
            return Err(StoreError::Conflict);
        }
        let next_revision = self.next_revision();
        values.insert(
            key.to_string(),
            StoredValue {
                revision: next_revision,
                value,
            },
        );
        Ok(next_revision)
    }

    async fn put(&self, key: &str, value: Bytes) -> Result<u64, StoreError> {
        let revision = self.next_revision();
        self.values
            .write()
            .await
            .insert(key.to_string(), StoredValue { revision, value });
        Ok(revision)
    }

    async fn keys(&self) -> Result<Vec<String>, StoreError> {
        Ok(self.values.read().await.keys().cloned().collect())
    }
}

#[derive(Clone)]
pub struct NatsStateStore {
    bucket: kv::Store,
}

impl NatsStateStore {
    pub fn new(bucket: kv::Store) -> Self {
        Self { bucket }
    }

    pub async fn ensure(
        jetstream: &jetstream::Context,
        bucket_name: &str,
        replicas: usize,
    ) -> Result<Self, StoreError> {
        let bucket = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: bucket_name.to_string(),
                description: "Durable worker run, step, worker, idempotency, signal, and lease-lane state"
                    .to_string(),
                history: 10,
                max_age: Duration::from_secs(0),
                storage: StorageType::File,
                num_replicas: replicas.max(1),
                ..Default::default()
            })
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(Self::new(bucket))
    }
}

#[async_trait]
impl StateStore for NatsStateStore {
    async fn get(&self, key: &str) -> Result<Option<StoredValue>, StoreError> {
        let entry = self
            .bucket
            .entry(key)
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(entry.and_then(|entry| {
            matches!(entry.operation, Operation::Put).then_some(StoredValue {
                revision: entry.revision,
                value: entry.value,
            })
        }))
    }

    async fn create(&self, key: &str, value: Bytes) -> Result<u64, StoreError> {
        self.bucket
            .create(key, value)
            .await
            .map_err(classify_write_error)
    }

    async fn update(&self, key: &str, revision: u64, value: Bytes) -> Result<u64, StoreError> {
        self.bucket
            .update(key, value, revision)
            .await
            .map_err(classify_write_error)
    }

    async fn put(&self, key: &str, value: Bytes) -> Result<u64, StoreError> {
        self.bucket
            .put(key, value)
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))
    }

    async fn keys(&self) -> Result<Vec<String>, StoreError> {
        let mut keys = self
            .bucket
            .keys()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        let mut result = Vec::new();
        while let Some(key) = keys
            .try_next()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?
        {
            result.push(key);
        }
        result.sort();
        Ok(result)
    }
}

#[async_trait]
pub trait EventSink: Send + Sync {
    async fn publish(&self, event: &DurableEvent) -> Result<(), StoreError>;
}

#[derive(Default)]
pub struct MemoryEventSink {
    events: Mutex<Vec<DurableEvent>>,
}

impl MemoryEventSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn snapshot(&self) -> Vec<DurableEvent> {
        self.events.lock().await.clone()
    }
}

#[async_trait]
impl EventSink for MemoryEventSink {
    async fn publish(&self, event: &DurableEvent) -> Result<(), StoreError> {
        let mut events = self.events.lock().await;
        if events.iter().all(|existing| existing.event_id != event.event_id) {
            events.push(event.clone());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct NatsEventSink {
    jetstream: jetstream::Context,
    subject_pattern: String,
}

impl NatsEventSink {
    pub fn new(jetstream: jetstream::Context, subject_pattern: impl Into<String>) -> Self {
        Self {
            jetstream,
            subject_pattern: subject_pattern.into(),
        }
    }

    pub async fn ensure_stream(
        jetstream: &jetstream::Context,
        stream_name: &str,
        subject: &str,
        replicas: usize,
    ) -> Result<(), StoreError> {
        jetstream
            .create_or_update_stream(StreamConfig {
                name: stream_name.to_string(),
                description: Some(
                    "Append-only durable worker lifecycle and streamed-output journal".to_string(),
                ),
                subjects: vec![subject.to_string()],
                retention: RetentionPolicy::Limits,
                storage: StorageType::File,
                max_age: Duration::from_secs(90 * 24 * 60 * 60),
                max_bytes: 20 * 1024 * 1024 * 1024,
                max_message_size: 2 * 1024 * 1024,
                num_replicas: replicas.max(1),
                duplicate_window: Duration::from_secs(10 * 60),
                ..Default::default()
            })
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl EventSink for NatsEventSink {
    async fn publish(&self, event: &DurableEvent) -> Result<(), StoreError> {
        let subject = self.subject_pattern.replace('*', &event.run_id);
        let payload = serde_json::to_vec(event)
            .map_err(|error| StoreError::Serialization(error.to_string()))?;
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Nats-Msg-Id", event.event_id.as_str());
        self.jetstream
            .publish_with_headers(subject, headers, payload.into())
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(())
    }
}

pub type SharedStore = Arc<dyn StateStore>;
pub type SharedEventSink = Arc<dyn EventSink>;
