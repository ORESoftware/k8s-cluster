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
const PING_NONCE_BYTES: usize = 16;
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
            .execute_raw(Statement::from_sql_and_values(
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

#[derive(Clone, Copy, Debug)]
struct PendingPong {
    payload: [u8; PING_NONCE_BYTES],
    deadline: TokioInstant,
}

#[derive(Debug, Default)]
struct PongTracker {
    pending: Option<PendingPong>,
}

impl PongTracker {
    fn expect(&mut self, payload: [u8; PING_NONCE_BYTES], now: TokioInstant, timeout: Duration) {
        debug_assert!(self.pending.is_none());
        self.pending = Some(PendingPong {
            payload,
            deadline: now + timeout,
        });
    }

    fn acknowledge(&mut self, payload: &[u8]) -> bool {
        let matches = self
            .pending
            .is_some_and(|pending| pending.payload.as_slice() == payload);
        if matches {
            self.pending = None;
        }
        matches
    }

    fn deadline(&self) -> Option<TokioInstant> {
        self.pending.map(|pending| pending.deadline)
    }

    fn is_waiting(&self) -> bool {
        self.pending.is_some()
    }
}

pub async fn serve(socket: WebSocket, actor: AuthContext, sessions: SessionService, hub: Hub) {
    serve_with_heartbeat_timing(
        socket,
        actor,
        sessions,
        hub,
        HEARTBEAT_INTERVAL,
        PONG_TIMEOUT,
    )
    .await;
}

async fn serve_with_heartbeat_timing(
    mut socket: WebSocket,
    actor: AuthContext,
    sessions: SessionService,
    hub: Hub,
    heartbeat_interval: Duration,
    pong_timeout: Duration,
) {
    let mut invalidations = hub.subscribe();
    let mut heartbeat = time::interval(heartbeat_interval);
    heartbeat.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    let mut pong_tracker = PongTracker::default();
    if send_json(
        &mut socket,
        &ServerMessage::Hello {
            protocol_version: 1,
            heartbeat_seconds: heartbeat_interval.as_secs(),
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
                    Some(Ok(Message::Pong(payload))) => {
                        pong_tracker.acknowledge(payload.as_ref());
                    }
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
            _ = pong_deadline_wait(pong_tracker.deadline()) => {
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
                // A delayed interval tick must never replace an unanswered
                // challenge and extend its deadline.
                if pong_tracker.is_waiting() {
                    continue;
                }
                let ping_payload = rand::random::<[u8; PING_NONCE_BYTES]>();
                if send_frame(&mut socket, Message::Ping(ping_payload.to_vec().into()))
                    .await
                    .is_err()
                {
                    break;
                }
                pong_tracker.expect(ping_payload, TokioInstant::now(), pong_timeout);
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
    use async_trait::async_trait;
    use axum::{
        extract::{ws::WebSocketUpgrade, State},
        http::{header, HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::get,
        Router,
    };
    use canonical_auth::{
        AuthProvider, AuthProviderError, AuthTokens, CredentialSource, SupabaseUser,
    };
    use canonical_session::SessionService;
    use chrono::Duration as ChronoDuration;
    use futures_util::StreamExt;
    use sea_orm::Database;
    use tokio::sync::oneshot;
    use tokio_tungstenite::tungstenite::{
        client::IntoClientRequest, protocol::frame::coding::CloseCode,
    };

    #[derive(Clone)]
    struct HeartbeatTestState {
        actor: AuthContext,
        sessions: SessionService,
        hub: Hub,
    }

    #[derive(Clone)]
    struct UnusedAuth;

    #[async_trait]
    impl AuthProvider for UnusedAuth {
        async fn password_sign_in(
            &self,
            _email: &str,
            _password: &str,
        ) -> Result<AuthTokens, AuthProviderError> {
            Err(AuthProviderError::Unavailable)
        }

        async fn refresh(&self, _refresh_token: &str) -> Result<AuthTokens, AuthProviderError> {
            Err(AuthProviderError::Unavailable)
        }

        async fn user_for_token(
            &self,
            _access_token: &str,
        ) -> Result<SupabaseUser, AuthProviderError> {
            Err(AuthProviderError::Unavailable)
        }

        async fn sign_out(&self, _access_token: &str) -> Result<(), AuthProviderError> {
            Err(AuthProviderError::Unavailable)
        }
    }

    async fn heartbeat_test_upgrade(
        State(state): State<HeartbeatTestState>,
        headers: HeaderMap,
        websocket: WebSocketUpgrade,
    ) -> Response {
        if headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some("Bearer heartbeat-test")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }

        let permit = state
            .hub
            .try_acquire_socket(state.actor.user_id)
            .expect("test socket capacity");
        websocket
            .on_upgrade(move |socket| async move {
                let _permit = permit;
                serve_with_heartbeat_timing(
                    socket,
                    state.actor,
                    state.sessions,
                    state.hub,
                    Duration::from_millis(20),
                    Duration::from_millis(50),
                )
                .await;
            })
            .into_response()
    }

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

    #[test]
    fn only_the_matching_pong_clears_the_liveness_deadline() {
        let now = TokioInstant::now();
        let timeout = Duration::from_millis(25);
        let expected = [7; PING_NONCE_BYTES];
        let mut tracker = PongTracker::default();
        tracker.expect(expected, now, timeout);

        assert_eq!(tracker.deadline(), Some(now + timeout));
        assert!(!tracker.acknowledge(&[8; PING_NONCE_BYTES]));
        assert_eq!(tracker.deadline(), Some(now + timeout));
        assert!(!tracker.acknowledge(&expected[..PING_NONCE_BYTES - 1]));
        assert_eq!(tracker.deadline(), Some(now + timeout));
        assert!(tracker.acknowledge(&expected));
        assert_eq!(tracker.deadline(), None);
    }

    #[tokio::test]
    async fn authenticated_socket_that_ignores_ping_is_closed_and_releases_capacity() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        let sessions = SessionService::new(
            db.clone(),
            Arc::new(UnusedAuth),
            &[7; 32],
            Duration::from_secs(60),
        )
        .unwrap();
        let owner_id = Uuid::new_v4();
        let hub = Hub::new(8);
        let state = HeartbeatTestState {
            actor: AuthContext {
                user_id: owner_id,
                email: "heartbeat@example.com".into(),
                source: CredentialSource::Bearer,
                supabase_session_id: None,
                session_hash: None,
                csrf_token: None,
                expires_at: Utc::now() + ChronoDuration::minutes(5),
            },
            sessions,
            hub: hub.clone(),
        };
        let app = Router::new()
            .route("/ws", get(heartbeat_test_upgrade))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        let mut request = format!("ws://{address}/ws").into_client_request().unwrap();
        request.headers_mut().insert(
            header::AUTHORIZATION,
            "Bearer heartbeat-test".parse().unwrap(),
        );
        let (mut socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

        // Do not poll the client while the Ping deadline elapses. This prevents
        // tungstenite from auto-queuing a Pong and exercises the server's real
        // timeout/close path over a TCP connection.
        time::sleep(Duration::from_millis(150)).await;
        let close = time::timeout(Duration::from_secs(1), async {
            loop {
                match socket.next().await {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(frame))) => break frame,
                    Some(Ok(_)) => {}
                    Some(Err(error)) => panic!("WebSocket read failed before close: {error}"),
                    None => panic!("WebSocket ended without a close frame"),
                }
            }
        })
        .await
        .expect("heartbeat close deadline")
        .expect("server close frame");
        assert_eq!(close.code, CloseCode::Policy);
        assert_eq!(close.reason, "heartbeat timeout");

        time::timeout(Duration::from_secs(1), async {
            loop {
                let released = {
                    let counts = hub.connections.lock().unwrap();
                    counts.total == 0 && !counts.by_owner.contains_key(&owner_id)
                };
                if released {
                    break;
                }
                time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("socket capacity permit should be released");

        let _ = shutdown_tx.send(());
        server.await.unwrap();
        db.close().await.unwrap();
    }
}
