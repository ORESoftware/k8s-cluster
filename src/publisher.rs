//! The NATS side of the bridge.
//!
//! The client slot is `Arc<RwLock<Option<Client>>>`, filled by a background
//! connect loop: the HTTP surface comes up immediately and reports
//! `/readyz` 503 until NATS is reachable (never a crash-loop on a NATS
//! outage — the tandem principle applied to messaging). Once connected,
//! async-nats reconnects on its own.
//!
//! Publishes `flush()` after sending so a 202 from `/publish` means the broker
//! actually received the bytes — plain core-NATS at-most-once. JetStream
//! durable publishes are the documented upgrade path for the sync outbox once
//! dd-nats enables it.

use std::sync::Arc;

use tokio::sync::RwLock;

/// Published (subject, payload) pairs recorded by the mock.
pub type MockLog = Arc<RwLock<Vec<(String, Vec<u8>)>>>;

#[derive(Clone)]
pub enum Publisher {
    Nats(Arc<RwLock<Option<async_nats::Client>>>),
    /// Tests: records published (subject, payload) pairs.
    #[allow(dead_code)]
    Mock(MockLog),
}

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error("nats not connected")]
    NotConnected,
    #[error("nats publish failed: {0}")]
    Failed(String),
}

impl Publisher {
    /// Start the background connect loop and return the shared slot.
    pub fn connect_in_background(nats_url: String) -> Self {
        let slot: Arc<RwLock<Option<async_nats::Client>>> = Arc::new(RwLock::new(None));
        let writer = slot.clone();
        tokio::spawn(async move {
            let mut delay = std::time::Duration::from_secs(1);
            loop {
                match async_nats::connect(&nats_url).await {
                    Ok(client) => {
                        tracing::info!(url = %nats_url, "connected to NATS");
                        *writer.write().await = Some(client);
                        return; // async-nats owns reconnection from here
                    }
                    Err(error) => {
                        tracing::warn!(url = %nats_url, %error, "NATS connect failed; retrying");
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(std::time::Duration::from_secs(30));
                    }
                }
            }
        });
        Publisher::Nats(slot)
    }

    #[allow(dead_code)]
    pub fn mock() -> Self {
        Publisher::Mock(Arc::new(RwLock::new(Vec::new())))
    }

    /// Broker-confirmed publish (`publish` + `flush`).
    pub async fn publish(&self, subject: &str, payload: Vec<u8>) -> Result<(), PublishError> {
        match self {
            Publisher::Nats(slot) => {
                let guard = slot.read().await;
                let client = guard.as_ref().ok_or(PublishError::NotConnected)?;
                client
                    .publish(subject.to_string(), payload.into())
                    .await
                    .map_err(|e| PublishError::Failed(e.to_string()))?;
                client
                    .flush()
                    .await
                    .map_err(|e| PublishError::Failed(e.to_string()))
            }
            Publisher::Mock(seen) => {
                seen.write().await.push((subject.to_string(), payload));
                Ok(())
            }
        }
    }

    /// Is the broker reachable right now? (readyz)
    pub async fn ready(&self) -> bool {
        match self {
            Publisher::Nats(slot) => match slot.read().await.as_ref() {
                Some(client) => {
                    client.connection_state() == async_nats::connection::State::Connected
                }
                None => false,
            },
            Publisher::Mock(_) => true,
        }
    }

    /// The connected client, for subscription loops.
    pub async fn client(&self) -> Option<async_nats::Client> {
        match self {
            Publisher::Nats(slot) => slot.read().await.clone(),
            Publisher::Mock(_) => None,
        }
    }
}
