use async_trait::async_trait;
use serde::Serialize;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::error::AppResult;

/// Context handed to a [`JobHandler`] when its kind is dispatched.
pub struct JobContext {
    pub pool: PgPool,
    pub job_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub kind: String,
    pub name: String,
    pub payload: serde_json::Value,
    pub attempt: i32,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct JobOutput {
    pub summary: String,
    pub data: serde_json::Value,
}

impl JobOutput {
    pub fn ok(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            data: serde_json::Value::Null,
        }
    }
    pub fn with_data(summary: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            summary: summary.into(),
            data,
        }
    }
}

#[async_trait]
pub trait JobHandler: Send + Sync + 'static {
    async fn run(&self, ctx: &JobContext) -> AppResult<JobOutput>;
}

/// Registry of `kind -> handler`. Cloneable; handlers are wrapped in Arc.
#[derive(Clone, Default)]
pub struct HandlerRegistry {
    inner: Arc<HashMap<String, Arc<dyn JobHandler>>>,
}

impl HandlerRegistry {
    pub fn from_map(map: HashMap<String, Arc<dyn JobHandler>>) -> Self {
        Self {
            inner: Arc::new(map),
        }
    }

    pub fn get(&self, kind: &str) -> Option<Arc<dyn JobHandler>> {
        self.inner.get(kind).cloned()
    }

    pub fn known_kinds(&self) -> Vec<String> {
        let mut v: Vec<String> = self.inner.keys().cloned().collect();
        v.sort();
        v
    }
}

pub struct HandlerRegistryBuilder {
    map: HashMap<String, Arc<dyn JobHandler>>,
}

impl HandlerRegistryBuilder {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn register(mut self, kind: impl Into<String>, handler: Arc<dyn JobHandler>) -> Self {
        self.map.insert(kind.into(), handler);
        self
    }

    pub fn build(self) -> HandlerRegistry {
        HandlerRegistry::from_map(self.map)
    }
}

impl Default for HandlerRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}
