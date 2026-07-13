use crate::auth::{AuthContext, SessionService};
use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket};
use chrono::Utc;
use sea_orm::{sqlx::postgres::PgListener, ConnectionTrait, DatabaseBackend, DbErr, Statement};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::{sync::broadcast, task::JoinHandle};
use uuid::Uuid;

pub const POSTGRES_INVALIDATION_CHANNEL: &str = "canonical_sync_invalidation_v1";
const BACKPLANE_VERSION: u8 = 1;
const MAX_BACKPLANE_PAYLOAD_BYTES: usize = 512;
const MIN_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);
const STABLE_LISTENER_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invalidation {
    pub owner_id: Uuid,
    pub cursor: i64,
}

#[derive(Clone)]
pub struct Hub {
    sender: broadcast::Sender<Invalidation>,
    instance_id: Uuid,
}

impl Hub {
    pub fn new(capacity: usize) -> Self {
        Self::with_instance_id(capacity, Uuid::new_v4())
    }

    fn with_instance_id(capacity: usize, instance_id: Uuid) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            instance_id,
        }
    }

    pub fn invalidate(&self, owner_id: Uuid, cursor: i64) {
        // No connected receivers is a normal state. REST pull is authoritative.
        let _ = self.sender.send(Invalidation { owner_id, cursor });
    }

    /// Queue a cross-instance hint in the same PostgreSQL transaction as the
    /// authoritative change. PostgreSQL releases NOTIFY messages only if that
    /// transaction commits; SQLite deliberately keeps the in-process path only.
    pub async fn enqueue_postgres_invalidation<C: ConnectionTrait>(
        &self,
        connection: &C,
        owner_id: Uuid,
        cursor: i64,
    ) -> Result<(), DbErr> {
        if connection.get_database_backend() != DatabaseBackend::Postgres {
            return Ok(());
        }

        let payload = self.backplane_payload(owner_id, cursor)?;
        connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT pg_notify($1, $2)",
                [
                    POSTGRES_INVALIDATION_CHANNEL.to_owned().into(),
                    payload.into(),
                ],
            ))
            .await?;
        Ok(())
    }

    fn backplane_payload(&self, owner_id: Uuid, cursor: i64) -> Result<String, DbErr> {
        if owner_id.is_nil() || cursor <= 0 {
            return Err(DbErr::Custom(
                "invalid internal sync invalidation".to_owned(),
            ));
        }
        let payload = serde_json::to_string(&BackplaneInvalidation {
            version: BACKPLANE_VERSION,
            source_instance: self.instance_id,
            owner_id,
            cursor,
        })
        .map_err(|_| DbErr::Custom("failed to serialize sync invalidation".to_owned()))?;
        if payload.len() > MAX_BACKPLANE_PAYLOAD_BYTES {
            return Err(DbErr::Custom(
                "internal sync invalidation exceeded its size limit".to_owned(),
            ));
        }
        Ok(payload)
    }

    fn relay_backplane_payload(&self, payload: &str) -> bool {
        if payload.len() > MAX_BACKPLANE_PAYLOAD_BYTES {
            tracing::warn!("ignored oversized PostgreSQL sync invalidation");
            return false;
        }
        let Ok(message) = serde_json::from_str::<BackplaneInvalidation>(payload) else {
            tracing::warn!("ignored malformed PostgreSQL sync invalidation");
            return false;
        };
        if message.version != BACKPLANE_VERSION
            || message.source_instance.is_nil()
            || message.owner_id.is_nil()
            || message.cursor <= 0
        {
            tracing::warn!("ignored invalid PostgreSQL sync invalidation");
            return false;
        }
        if message.source_instance == self.instance_id {
            return false;
        }
        self.invalidate(message.owner_id, message.cursor);
        true
    }

    fn subscribe(&self) -> broadcast::Receiver<Invalidation> {
        self.sender.subscribe()
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackplaneInvalidation {
    version: u8,
    source_instance: Uuid,
    owner_id: Uuid,
    cursor: i64,
}

/// Owns the optional PostgreSQL listener task. Dropping it during graceful
/// server shutdown aborts a pending receive or reconnect sleep immediately.
pub struct BackplaneTask(JoinHandle<()>);

impl Drop for BackplaneTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub fn spawn_postgres_backplane(database_url: String, hub: Hub) -> BackplaneTask {
    BackplaneTask(tokio::spawn(async move {
        run_postgres_backplane(database_url, hub).await;
    }))
}

async fn run_postgres_backplane(database_url: String, hub: Hub) {
    let mut reconnect_delay = MIN_RECONNECT_DELAY;
    loop {
        let started_at = Instant::now();
        if listen_once(&database_url, &hub).await.is_err() {
            if started_at.elapsed() >= STABLE_LISTENER_WINDOW {
                reconnect_delay = MIN_RECONNECT_DELAY;
            }
            let delay = reconnect_delay;
            tracing::warn!(
                retry_seconds = delay.as_secs(),
                "PostgreSQL sync invalidation listener disconnected; retrying"
            );
            reconnect_delay = reconnect_delay.saturating_mul(2).min(MAX_RECONNECT_DELAY);
            tokio::time::sleep(delay).await;
        }
    }
}

async fn listen_once(database_url: &str, hub: &Hub) -> Result<(), sea_orm::sqlx::Error> {
    let mut listener = PgListener::connect(database_url).await?;
    listener.listen(POSTGRES_INVALIDATION_CHANNEL).await?;
    tracing::info!(
        channel = POSTGRES_INVALIDATION_CHANNEL,
        "PostgreSQL sync invalidation listener ready"
    );
    loop {
        let notification = listener.recv().await?;
        if notification.channel() == POSTGRES_INVALIDATION_CHANNEL {
            hub.relay_backplane_payload(notification.payload());
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum ServerMessage {
    #[serde(rename = "hello")]
    Hello {
        #[serde(rename = "protocolVersion")]
        protocol_version: u8,
        #[serde(rename = "heartbeatSeconds")]
        heartbeat_seconds: u64,
    },
    #[serde(rename = "sync.invalidated")]
    SyncInvalidated {
        #[serde(rename = "latestHint")]
        latest_hint: String,
    },
    #[serde(rename = "resync_required")]
    ResyncRequired,
    #[serde(rename = "pong")]
    Pong,
}

pub async fn serve(mut socket: WebSocket, actor: AuthContext, sessions: SessionService, hub: Hub) {
    let mut invalidations = hub.subscribe();
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(25));
    if send_json(
        &mut socket,
        &ServerMessage::Hello {
            protocol_version: 1,
            heartbeat_seconds: 25,
        },
    )
    .await
    .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if text.contains("\"type\":\"ping\"")
                            && send_json(&mut socket, &ServerMessage::Pong).await.is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(Message::Binary(_))) | Some(Ok(Message::Pong(_))) => {}
                }
            }
            event = invalidations.recv() => {
                match event {
                    Ok(event) if event.owner_id == actor.user_id => {
                        if send_json(
                            &mut socket,
                            &ServerMessage::SyncInvalidated {
                                latest_hint: event.cursor.to_string(),
                            },
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if send_json(&mut socket, &ServerMessage::ResyncRequired).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = heartbeat.tick() => {
                if Utc::now() >= actor.expires_at || sessions.revalidate(&actor).await.is_err() {
                    let _ = socket
                        .send(Message::Close(Some(CloseFrame {
                            code: close_code::POLICY,
                            reason: "session expired".into(),
                        })))
                        .await;
                    break;
                }
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
        }
    }
}

async fn send_json(socket: &mut WebSocket, message: &ServerMessage) -> Result<(), ()> {
    let text = serde_json::to_string(message).map_err(|_| ())?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn backplane_payloads_are_bounded_strict_and_self_deduplicated() {
        let local_instance = Uuid::from_u128(1);
        let remote_instance = Uuid::from_u128(2);
        let owner_id = Uuid::from_u128(3);
        let local = Hub::with_instance_id(8, local_instance);
        let remote = Hub::with_instance_id(8, remote_instance);
        let mut receiver = local.subscribe();

        let payload = remote.backplane_payload(owner_id, 41).unwrap();
        assert!(payload.len() <= MAX_BACKPLANE_PAYLOAD_BYTES);
        assert!(local.relay_backplane_payload(&payload));
        assert_eq!(
            receiver.recv().await.unwrap(),
            Invalidation {
                owner_id,
                cursor: 41
            }
        );

        let self_payload = local.backplane_payload(owner_id, 42).unwrap();
        assert!(!local.relay_backplane_payload(&self_payload));
        assert!(matches!(
            receiver.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        let with_unknown_field = payload.replacen("}", ",\"unexpected\":true}", 1);
        assert!(!local.relay_backplane_payload(&with_unknown_field));
        assert!(!local.relay_backplane_payload(&"x".repeat(MAX_BACKPLANE_PAYLOAD_BYTES + 1)));
    }
}
