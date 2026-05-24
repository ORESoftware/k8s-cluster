//! Tokio-based clients for the broker's TCP/UDS protocol.
//!
//! `Client` exposes exclusive locking (`acquire` / `release`, single-key or
//! composite). `RwClient` exposes reader-writer locking (`acquire_read`,
//! `acquire_write`, plus `release`).
//!
//! Both clients open one connection and multiplex many in-flight requests
//! over it via correlation UUIDs. A background reader task fans responses
//! out to per-request `mpsc::UnboundedSender` channels — `mpsc` rather than
//! `oneshot` because the broker may send multiple responses for one request
//! UUID (a "queued" notice followed by the eventual "acquired" grant).
//!
//! Acquire calls block until either a definitive response (`acquired:true`)
//! is received or the supplied timeout elapses. Callers wanting a single
//! "queued" notice without waiting for the grant should use `try_acquire`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, UnixStream};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::protocol::{Request, Response, MAX_COMPOSITE_KEYS};

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("encoding error: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("broker reported error: {0}")]
    Broker(String),
    #[error("operation timed out after {0:?}")]
    Timeout(Duration),
    #[error("client transport closed")]
    Closed,
    #[error("invalid request: {0}")]
    Invalid(String),
}

type Inflight = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Response>>>>;

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub auth_token: Option<String>,
    pub default_request_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            auth_token: None,
            default_request_timeout: Duration::from_secs(5),
        }
    }
}

/// Connection to a broker. Cheap to clone; all clones share the same socket.
#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    inflight: Inflight,
    writer: tokio::sync::Mutex<Box<dyn AsyncSend + Send + Unpin>>,
    config: ClientConfig,
    /// Aborts the spawned reader task when the last `Arc<ClientInner>`
    /// drops. Without this, `tokio::io::split` keeps the underlying
    /// stream alive (both halves hold a reference) so the broker never
    /// sees EOF and never runs `drop_client` for this connection.
    /// Aborting drops the reader's `ReadHalf`, which combined with the
    /// `WriteHalf` going away when `writer` drops, closes the socket.
    reader_task: tokio::task::AbortHandle,
}

impl Drop for ClientInner {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

trait AsyncSend: tokio::io::AsyncWrite {}
impl<T: tokio::io::AsyncWrite> AsyncSend for T {}

impl Client {
    pub async fn connect_tcp(
        addr: impl tokio::net::ToSocketAddrs,
        config: ClientConfig,
    ) -> Result<Self, ClientError> {
        let stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true).ok();
        Self::start(stream, config).await
    }

    pub async fn connect_uds(
        path: impl AsRef<Path>,
        config: ClientConfig,
    ) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path.as_ref()).await?;
        Self::start(stream, config).await
    }

    async fn start<S>(stream: S, config: ClientConfig) -> Result<Self, ClientError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        let (read, write) = tokio::io::split(stream);
        let inflight: Inflight = Arc::new(Mutex::new(HashMap::new()));
        let inflight_reader = inflight.clone();
        let reader_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(read);
            let mut buf = String::new();
            loop {
                buf.clear();
                let n = match reader.read_line(&mut buf).await {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if n == 0 {
                    break;
                }
                let line = buf.trim();
                if line.is_empty() {
                    continue;
                }
                let resp: Response = match serde_json::from_str(line) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let uuid = resp.correlation_uuid().to_string();
                let waiter = inflight_reader.lock().get(&uuid).cloned();
                if let Some(tx) = waiter {
                    let _ = tx.send(resp);
                }
            }
            let mut guard = inflight_reader.lock();
            for (uuid, tx) in guard.drain() {
                let _ = tx.send(Response::Error {
                    uuid,
                    error: "broker connection closed".into(),
                });
            }
        });

        let client = Self {
            inner: Arc::new(ClientInner {
                inflight,
                writer: tokio::sync::Mutex::new(Box::new(write)),
                config: config.clone(),
                reader_task: reader_handle.abort_handle(),
            }),
        };

        let version_uuid = Uuid::new_v4().to_string();
        let mut rx = client.register_inflight(&version_uuid);
        client
            .write_request(Request::Version {
                uuid: version_uuid.clone(),
                value: crate::protocol::PROTOCOL_VERSION.into(),
            })
            .await?;
        let _ = client
            .recv_one(&mut rx, &version_uuid, Duration::from_secs(2))
            .await;
        client.unregister_inflight(&version_uuid);

        if let Some(token) = config.auth_token.as_ref() {
            let auth_uuid = Uuid::new_v4().to_string();
            let mut rx = client.register_inflight(&auth_uuid);
            client
                .write_request(Request::Auth {
                    uuid: auth_uuid.clone(),
                    token: token.clone(),
                })
                .await?;
            let resp = client
                .recv_one(&mut rx, &auth_uuid, Duration::from_secs(2))
                .await?;
            client.unregister_inflight(&auth_uuid);
            match resp {
                Response::Auth { ok: true, .. } => {}
                Response::Auth {
                    ok: false, error, ..
                } => {
                    return Err(ClientError::Broker(
                        error.unwrap_or_else(|| "auth failed".into()),
                    ))
                }
                other => {
                    return Err(ClientError::Broker(format!(
                        "unexpected auth response: {other:?}"
                    )))
                }
            }
        }
        Ok(client)
    }

    pub async fn acquire(&self, key: &str, ttl: Duration) -> Result<LockGuard, ClientError> {
        self.acquire_internal(Some(key.to_string()), None, ttl, None).await
    }

    /// Acquire a semaphore-style lock allowing up to `max` simultaneous
    /// holders on `key`. Each holder still receives a unique
    /// `lock_uuid` and a unique fencing token, so callers can
    /// distinguish slot-N from slot-M without coordinating.
    ///
    /// `max == 1` is equivalent to [`Client::acquire`]. The broker
    /// silently clamps `max` to its `max_concurrency_cap`
    /// (default `100`, see
    /// [`crate::protocol::DEFAULT_MAX_CONCURRENCY_CAP`] and
    /// `LMX_MAX_CONCURRENCY_CAP`); the clamp is observable via the
    /// `dd_rust_network_mutex_concurrency_cap_clamps_total` Prometheus
    /// counter.
    ///
    /// `max == 0` is rejected immediately as
    /// [`ClientError::Invalid`] — there's no defensible "zero
    /// concurrent holders" semantic, and silently treating it as the
    /// default would mask bugs in caller code. The broker enforces the
    /// same rule for cross-runtime clients that bypass this helper:
    /// raw TCP/UDS or HTTP requests with `max: 0` come back as a
    /// non-acquired response with a clear `error` field rather than a
    /// silent grant.
    pub async fn acquire_with_max(
        &self,
        key: &str,
        max: u32,
        ttl: Duration,
    ) -> Result<LockGuard, ClientError> {
        if max == 0 {
            return Err(ClientError::Invalid(
                "acquire_with_max requires max >= 1; use acquire() for default semantics".into(),
            ));
        }
        self.acquire_internal(Some(key.to_string()), None, ttl, Some(max))
            .await
    }

    pub async fn acquire_composite(
        &self,
        keys: &[&str],
        ttl: Duration,
    ) -> Result<LockGuard, ClientError> {
        if keys.is_empty() || keys.len() > MAX_COMPOSITE_KEYS {
            return Err(ClientError::Invalid(format!(
                "composite acquire requires 1..={MAX_COMPOSITE_KEYS} keys"
            )));
        }
        self.acquire_internal(
            None,
            Some(keys.iter().map(|s| s.to_string()).collect()),
            ttl,
            None,
        )
        .await
    }

    async fn acquire_internal(
        &self,
        key: Option<String>,
        keys: Option<Vec<String>>,
        ttl: Duration,
        max: Option<u32>,
    ) -> Result<LockGuard, ClientError> {
        let request_uuid = Uuid::new_v4().to_string();
        let mut rx = self.register_inflight(&request_uuid);

        let request = Request::Lock {
            uuid: request_uuid.clone(),
            key: key.clone(),
            keys: keys.clone(),
            pid: Some(std::process::id() as i64),
            ttl: Some(ttl.as_millis() as u64),
            max,
            force: false,
            retry_count: 0,
            keep_locks_after_death: false,
        };
        if let Err(err) = self.write_request(request).await {
            self.unregister_inflight(&request_uuid);
            return Err(err);
        }

        let timeout = self.inner.config.default_request_timeout;
        let result = self.wait_for_acquire(&mut rx, &request_uuid, timeout).await;
        self.unregister_inflight(&request_uuid);
        result
    }

    async fn wait_for_acquire(
        &self,
        rx: &mut mpsc::UnboundedReceiver<Response>,
        request_uuid: &str,
        timeout: Duration,
    ) -> Result<LockGuard, ClientError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(ClientError::Timeout(timeout));
            }
            let resp = match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(resp)) => resp,
                Ok(None) => return Err(ClientError::Closed),
                Err(_) => return Err(ClientError::Timeout(timeout)),
            };
            match resp {
                Response::Lock {
                    acquired: true,
                    key,
                    lock_uuid: Some(lock_uuid),
                    fencing_token,
                    ..
                } => return Ok(LockGuard::single(key, lock_uuid, fencing_token)),
                Response::Lock {
                    acquired: false, ..
                } => continue,
                Response::CompositeLock {
                    acquired: true,
                    keys,
                    lock_uuid: Some(lock_uuid),
                    fencing_tokens,
                    ..
                } => {
                    return Ok(LockGuard::composite(
                        keys,
                        lock_uuid,
                        fencing_tokens.unwrap_or_default(),
                    ));
                }
                Response::CompositeLock {
                    acquired: false, ..
                } => continue,
                Response::Error { error, .. } => return Err(ClientError::Broker(error)),
                _ => {
                    let _ = request_uuid;
                    continue;
                }
            }
        }
    }

    pub async fn release(&self, guard: &LockGuard) -> Result<(), ClientError> {
        let request_uuid = Uuid::new_v4().to_string();
        let mut rx = self.register_inflight(&request_uuid);
        let request = Request::Unlock {
            uuid: request_uuid.clone(),
            key: if guard.keys.len() == 1 {
                Some(guard.keys[0].clone())
            } else {
                None
            },
            keys: if guard.keys.len() > 1 {
                Some(guard.keys.clone())
            } else {
                None
            },
            lock_uuid: Some(guard.lock_uuid.clone()),
            force: false,
        };
        let outcome = self.roundtrip(request, &request_uuid, &mut rx).await;
        self.unregister_inflight(&request_uuid);
        match outcome? {
            Response::Unlock {
                unlocked: true, ..
            } => Ok(()),
            Response::Unlock {
                unlocked: false,
                error,
                ..
            } => Err(ClientError::Broker(
                error.unwrap_or_else(|| "unlock returned unlocked=false".into()),
            )),
            Response::Error { error, .. } => Err(ClientError::Broker(error)),
            other => Err(ClientError::Broker(format!(
                "unexpected unlock response: {other:?}"
            ))),
        }
    }

    pub async fn lock_info(&self, key: &str) -> Result<LockInfo, ClientError> {
        let request_uuid = Uuid::new_v4().to_string();
        let mut rx = self.register_inflight(&request_uuid);
        let request = Request::LockInfo {
            uuid: request_uuid.clone(),
            key: key.to_string(),
        };
        let outcome = self.roundtrip(request, &request_uuid, &mut rx).await;
        self.unregister_inflight(&request_uuid);
        match outcome? {
            Response::LockInfo {
                key,
                is_locked,
                lockholder_uuids,
                lock_request_count,
                readers_count,
                writer_flag,
                ..
            } => Ok(LockInfo {
                key,
                is_locked,
                lockholder_uuids,
                lock_request_count,
                readers_count,
                writer_flag,
            }),
            Response::Error { error, .. } => Err(ClientError::Broker(error)),
            other => Err(ClientError::Broker(format!("unexpected: {other:?}"))),
        }
    }

    pub async fn ls(&self) -> Result<Vec<String>, ClientError> {
        let request_uuid = Uuid::new_v4().to_string();
        let mut rx = self.register_inflight(&request_uuid);
        let outcome = self
            .roundtrip(
                Request::Ls {
                    uuid: request_uuid.clone(),
                },
                &request_uuid,
                &mut rx,
            )
            .await;
        self.unregister_inflight(&request_uuid);
        match outcome? {
            Response::LsResult { keys, .. } => Ok(keys),
            other => Err(ClientError::Broker(format!("unexpected: {other:?}"))),
        }
    }

    fn register_inflight(&self, uuid: &str) -> mpsc::UnboundedReceiver<Response> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.inflight.lock().insert(uuid.to_string(), tx);
        rx
    }

    fn unregister_inflight(&self, uuid: &str) {
        self.inner.inflight.lock().remove(uuid);
    }

    async fn write_request(&self, request: Request) -> Result<(), ClientError> {
        let mut bytes = serde_json::to_vec(&request)?;
        bytes.push(b'\n');
        let mut writer = self.inner.writer.lock().await;
        writer.write_all(&bytes).await?;
        writer.flush().await?;
        Ok(())
    }

    async fn recv_one(
        &self,
        rx: &mut mpsc::UnboundedReceiver<Response>,
        _uuid: &str,
        timeout: Duration,
    ) -> Result<Response, ClientError> {
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(resp)) => Ok(resp),
            Ok(None) => Err(ClientError::Closed),
            Err(_) => Err(ClientError::Timeout(timeout)),
        }
    }

    async fn roundtrip(
        &self,
        request: Request,
        request_uuid: &str,
        rx: &mut mpsc::UnboundedReceiver<Response>,
    ) -> Result<Response, ClientError> {
        self.write_request(request).await?;
        self.recv_one(rx, request_uuid, self.inner.config.default_request_timeout)
            .await
    }
}

/// Reader-writer client. Owns its own connection. Acquired guards drop into
/// release calls automatically when `release()` is called.
#[derive(Clone)]
pub struct RwClient {
    inner: Client,
}

impl RwClient {
    pub async fn connect_tcp(
        addr: impl tokio::net::ToSocketAddrs,
        config: ClientConfig,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            inner: Client::connect_tcp(addr, config).await?,
        })
    }

    pub async fn connect_uds(
        path: impl AsRef<Path>,
        config: ClientConfig,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            inner: Client::connect_uds(path, config).await?,
        })
    }

    pub async fn acquire_read(&self, key: &str) -> Result<RwReadGuard, ClientError> {
        let request_uuid = Uuid::new_v4().to_string();
        let mut rx = self.inner.register_inflight(&request_uuid);
        let send = self
            .inner
            .write_request(Request::RegisterRead {
                uuid: request_uuid.clone(),
                key: key.to_string(),
            })
            .await;
        if let Err(err) = send {
            self.inner.unregister_inflight(&request_uuid);
            return Err(err);
        }
        let result = self.wait_for_rw_grant(&mut rx, true, key).await;
        self.inner.unregister_inflight(&request_uuid);
        let (lock_uuid, fencing_token) = result?;
        Ok(RwReadGuard {
            client: self.inner.clone(),
            key: key.to_string(),
            lock_uuid,
            fencing_token,
        })
    }

    pub async fn acquire_write(&self, key: &str) -> Result<RwWriteGuard, ClientError> {
        let request_uuid = Uuid::new_v4().to_string();
        let mut rx = self.inner.register_inflight(&request_uuid);
        let send = self
            .inner
            .write_request(Request::RegisterWrite {
                uuid: request_uuid.clone(),
                key: key.to_string(),
            })
            .await;
        if let Err(err) = send {
            self.inner.unregister_inflight(&request_uuid);
            return Err(err);
        }
        let result = self.wait_for_rw_grant(&mut rx, false, key).await;
        self.inner.unregister_inflight(&request_uuid);
        let (lock_uuid, fencing_token) = result?;
        Ok(RwWriteGuard {
            client: self.inner.clone(),
            key: key.to_string(),
            lock_uuid,
            fencing_token,
        })
    }

    async fn wait_for_rw_grant(
        &self,
        rx: &mut mpsc::UnboundedReceiver<Response>,
        is_read: bool,
        _key: &str,
    ) -> Result<(String, Option<u64>), ClientError> {
        let timeout = self.inner.inner.config.default_request_timeout;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(ClientError::Timeout(timeout));
            }
            let resp = match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(r)) => r,
                Ok(None) => return Err(ClientError::Closed),
                Err(_) => return Err(ClientError::Timeout(timeout)),
            };
            match (is_read, resp) {
                (
                    true,
                    Response::RegisterReadResult {
                        granted: true,
                        lock_uuid: Some(u),
                        fencing_token,
                        ..
                    },
                )
                | (
                    false,
                    Response::RegisterWriteResult {
                        granted: true,
                        lock_uuid: Some(u),
                        fencing_token,
                        ..
                    },
                ) => return Ok((u, fencing_token)),
                (_, Response::RegisterReadResult { granted: false, .. })
                | (_, Response::RegisterWriteResult { granted: false, .. }) => continue,
                (_, Response::Error { error, .. }) => return Err(ClientError::Broker(error)),
                _ => continue,
            }
        }
    }
}

/// Acquired exclusive (or composite) lock token. Caller must explicitly call
/// `Client::release` when done — Rust's Drop semantics can't safely run async
/// release, and silent leaks beat surprise blocking inside `Drop`.
#[derive(Debug, Clone)]
pub struct LockGuard {
    pub keys: Vec<String>,
    pub lock_uuid: String,
    pub fencing_token: Option<u64>,
    pub fencing_tokens: BTreeMap<String, u64>,
}

impl LockGuard {
    fn single(key: String, lock_uuid: String, fencing_token: Option<u64>) -> Self {
        let mut tokens = BTreeMap::new();
        if let Some(t) = fencing_token {
            tokens.insert(key.clone(), t);
        }
        Self {
            keys: vec![key],
            lock_uuid,
            fencing_token,
            fencing_tokens: tokens,
        }
    }

    fn composite(
        keys: Vec<String>,
        lock_uuid: String,
        fencing_tokens: BTreeMap<String, u64>,
    ) -> Self {
        Self {
            keys,
            lock_uuid,
            fencing_token: None,
            fencing_tokens,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LockInfo {
    pub key: String,
    pub is_locked: bool,
    pub lockholder_uuids: Vec<String>,
    pub lock_request_count: usize,
    pub readers_count: u32,
    pub writer_flag: bool,
}

pub struct RwReadGuard {
    client: Client,
    pub key: String,
    pub lock_uuid: String,
    pub fencing_token: Option<u64>,
}

impl RwReadGuard {
    pub async fn release(self) -> Result<(), ClientError> {
        self.client
            .release(&LockGuard::single(
                self.key.clone(),
                self.lock_uuid.clone(),
                self.fencing_token,
            ))
            .await
    }
}

pub struct RwWriteGuard {
    client: Client,
    pub key: String,
    pub lock_uuid: String,
    pub fencing_token: Option<u64>,
}

impl RwWriteGuard {
    pub async fn release(self) -> Result<(), ClientError> {
        self.client
            .release(&LockGuard::single(
                self.key.clone(),
                self.lock_uuid.clone(),
                self.fencing_token,
            ))
            .await
    }
}

impl Client {
    /// Read-only accessor for the configured `ClientConfig`.
    pub fn config(&self) -> &ClientConfig {
        &self.inner.config
    }
}
