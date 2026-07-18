use crate::auth::{AuthContext, SessionService};
use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket};
use chrono::Utc;
use sea_orm::{sqlx::postgres::PgListener, ConnectionTrait, DatabaseBackend, DbErr, Statement};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};
use tokio::{
    sync::broadcast,
    task::JoinHandle,
    time::{self, Instant as TokioInstant},
};
use uuid::Uuid;

pub const POSTGRES_INVALIDATION_CHANNEL: &str = "canonical_sync_invalidation_v1";
const BACKPLANE_VERSION: u8 = 1;
const MAX_BACKPLANE_PAYLOAD_BYTES: usize = 512;
const MIN_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);
const STABLE_LISTENER_WINDOW: Duration = Duration::from_secs(60);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);
const PONG_TIMEOUT: Duration = Duration::from_secs(10);
const SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SOCKETS_PER_USER: usize = 4;
const MAX_SOCKETS_PER_INSTANCE: usize = 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invalidation {
    pub owner_id: Uuid,
    pub cursor: i64,
}

#[derive(Clone)]
pub struct Hub {
    sender: broadcast::Sender<Invalidation>,
    instance_id: Uuid,
    connections: Arc<StdMutex<ConnectionCounts>>,
}

#[derive(Default)]
struct ConnectionCounts {
    total: usize,
    by_owner: HashMap<Uuid, usize>,
}

/// Keeps a WebSocket capacity reservation alive until its connection closes.
pub struct SocketPermit {
    connections: Arc<StdMutex<ConnectionCounts>>,
    owner_id: Uuid,
}

impl Drop for SocketPermit {
    fn drop(&mut self) {
        let mut counts = self
            .connections
            .lock()
            .expect("WebSocket connection counter lock poisoned");
        counts.total = counts.total.saturating_sub(1);
        if let Some(owner_count) = counts.by_owner.get_mut(&self.owner_id) {
            *owner_count = owner_count.saturating_sub(1);
            if *owner_count == 0 {
                counts.by_owner.remove(&self.owner_id);
            }
        }
    }
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
            connections: Arc::new(StdMutex::new(ConnectionCounts::default())),
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

    pub fn try_acquire_socket(&self, owner_id: Uuid) -> Option<SocketPermit> {
        let mut counts = self
            .connections
            .lock()
            .expect("WebSocket connection counter lock poisoned");
        let owner_count = counts.by_owner.get(&owner_id).copied().unwrap_or_default();
        if counts.total >= MAX_SOCKETS_PER_INSTANCE || owner_count >= MAX_SOCKETS_PER_USER {
            return None;
        }
        counts.total += 1;
        counts.by_owner.insert(owner_id, owner_count + 1);
        Some(SocketPermit {
            connections: self.connections.clone(),
            owner_id,
        })
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
    let mut heartbeat = time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    let mut pong_deadline = None;
    if send_json(
        &mut socket,
        &ServerMessage::Hello {
            protocol_version: 1,
            heartbeat_seconds: HEARTBEAT_INTERVAL.as_secs(),
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
                        if send_frame(&mut socket, Message::Pong(payload)).await.is_err() {
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
                    Some(Ok(Message::Pong(_))) => pong_deadline = None,
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(Message::Binary(_))) => {}
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
            _ = pong_deadline_wait(pong_deadline) => {
                let _ = send_frame(
                    &mut socket,
                    Message::Close(Some(CloseFrame {
                        code: close_code::POLICY,
                        reason: "heartbeat timeout".into(),
                    })),
                ).await;
                break;
            }
            _ = heartbeat.tick() => {
                if Utc::now() >= actor.expires_at || sessions.revalidate(&actor).await.is_err() {
                    let _ = send_frame(
                        &mut socket,
                        Message::Close(Some(CloseFrame {
                            code: close_code::POLICY,
                            reason: "session expired".into(),
                        })),
                    )
                    .await;
                    break;
                }
                if send_frame(&mut socket, Message::Ping(Vec::new().into()))
                    .await
                    .is_err()
                {
                    break;
                }
                pong_deadline = Some(TokioInstant::now() + PONG_TIMEOUT);
            }
        }
    }
}

async fn send_json(socket: &mut WebSocket, message: &ServerMessage) -> Result<(), ()> {
    let text = serde_json::to_string(message).map_err(|_| ())?;
    send_frame(socket, Message::Text(text.into())).await
}

async fn send_frame(socket: &mut WebSocket, message: Message) -> Result<(), ()> {
    time::timeout(SOCKET_WRITE_TIMEOUT, socket.send(message))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

async fn pong_deadline_wait(deadline: Option<TokioInstant>) {
    match deadline {
        Some(deadline) => time::sleep_until(deadline).await,
        None => std::future::pending::<()>().await,
    }
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

    #[test]
    fn socket_capacity_is_bounded_per_user_and_released_on_close() {
        let hub = Hub::new(8);
        let owner = Uuid::new_v4();
        let mut permits = Vec::new();
        for _ in 0..MAX_SOCKETS_PER_USER {
            permits.push(hub.try_acquire_socket(owner).unwrap());
        }
        assert!(hub.try_acquire_socket(owner).is_none());
        drop(permits.pop());
        assert!(hub.try_acquire_socket(owner).is_some());
    }
}
