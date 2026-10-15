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
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
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

const DEFAULT_MAX_RESPONSE_FRAME_BYTES: usize = 1024 * 1024;

fn max_response_frame_bytes() -> usize {
    crate::routine_id!("ddl-routine-client-max-frame-bytes-T2b");
    std::env::var("LMX_MAX_RESPONSE_FRAME_BYTES")
        .or_else(|_| std::env::var("LMX_MAX_FRAME_BYTES"))
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_RESPONSE_FRAME_BYTES)
}

async fn read_response_frame_bounded<R>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max_bytes: usize,
) -> std::io::Result<bool>
where
    R: AsyncBufRead + Unpin,
{
    crate::routine_id!("ddl-routine-client-read-frame-bounded-Q8k");
    buf.clear();
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            if buf.is_empty() {
                return Ok(false);
            }
            return Ok(true);
        }

        if let Some(idx) = chunk.iter().position(|&b| b == b'\n') {
            let take = idx + 1;
            if buf.len() + take > max_bytes {
                reader.consume(take);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("response frame exceeds {max_bytes} bytes"),
                ));
            }
            buf.extend_from_slice(&chunk[..take]);
            reader.consume(take);
            return Ok(true);
        }

        let take = chunk.len();
        if buf.len() + take > max_bytes {
            reader.consume(take);
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("response frame exceeds {max_bytes} bytes"),
            ));
        }
        buf.extend_from_slice(chunk);
        reader.consume(take);
    }
}

fn trim_response_frame(buf: &[u8]) -> &[u8] {
    crate::routine_id!("ddl-routine-client-trim-frame-G2c");
    let mut end = buf.len();
    while end > 0 && (buf[end - 1] == b'\n' || buf[end - 1] == b'\r') {
        end -= 1;
    }
    &buf[..end]
}

fn deadline_after(timeout: Duration) -> tokio::time::Instant {
    crate::routine_id!("ddl-routine-client-deadline-after-f6K");
    let now = tokio::time::Instant::now();
    now.checked_add(timeout)
        .unwrap_or_else(|| now + Duration::from_secs(365 * 24 * 60 * 60))
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub auth_token: Option<String>,
    pub default_request_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        crate::routine_id!("ddl-routine-h0o70WdF73Tn1IM1HU");
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
        crate::routine_id!("ddl-routine-ABoMt9DWOh9F9GM4hA");
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
        crate::routine_id!("ddl-routine-Vs4WhHDADTirOfwOaP");
        let stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true).ok();
        Self::start(stream, config).await
    }

    pub async fn connect_uds(
        path: impl AsRef<Path>,
        config: ClientConfig,
    ) -> Result<Self, ClientError> {
        crate::routine_id!("ddl-routine-rymSP7H4L8S6yqNSir");
        let stream = UnixStream::connect(path.as_ref()).await?;
        Self::start(stream, config).await
    }

    async fn start<S>(stream: S, config: ClientConfig) -> Result<Self, ClientError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        crate::routine_id!("ddl-routine-U8KUwlqwY8R2TlhiK8");
        let (read, write) = tokio::io::split(stream);
        let inflight: Inflight = Arc::new(Mutex::new(HashMap::new()));
        let inflight_reader = inflight.clone();
        let reader_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(read);
            let mut buf = Vec::new();
            let frame_cap = max_response_frame_bytes();
            loop {
                let got_frame =
                    match read_response_frame_bounded(&mut reader, &mut buf, frame_cap).await {
                        Ok(got_frame) => got_frame,
                        Err(_) => break,
                    };
                if !got_frame {
                    break;
                }
                let payload = trim_response_frame(&buf);
                if payload.is_empty() {
                    continue;
                }
                let resp: Response = match serde_json::from_slice(payload) {
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
        crate::routine_id!("ddl-routine-d2XKMm8wCgREqHR4iw");
        self.acquire_internal(Some(key.to_string()), None, ttl, None)
            .await
    }

    /// Acquire a semaphore-style lock allowing up to `max` simultaneous
    /// holders on `key`. Each holder still receives a unique
    /// `lock_uuid` and a unique fencing token, so callers can
    /// distinguish slot-N from slot-M without coordinating.
    ///
    /// `max == 1` is equivalent to [`Client::acquire`]. The broker
    /// silently clamps `max` to its `max_concurrency_cap`
    /// (default `1_000`, see
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
        crate::routine_id!("ddl-routine---QRAg2Rtms9lwn768");
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
        crate::routine_id!("ddl-routine--MNPLcy68N9Ksp7Y52");
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
        crate::routine_id!("ddl-routine-E2WC66KPw4xPllqeal");
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
            wait: Some(true),
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

    /// Non-blocking single-key acquire. Returns `Ok(None)` immediately if the
    /// key is currently contended (the broker does not enqueue the request, so
    /// nothing is leaked). Use [`Self::acquire`] for the blocking variant.
    pub async fn try_acquire(
        &self,
        key: &str,
        ttl: Duration,
    ) -> Result<Option<LockGuard>, ClientError> {
        crate::routine_id!("ddl-routine-tryacq-single-7Qp");
        self.try_acquire_internal(Some(key.to_string()), None, ttl, None)
            .await
    }

    /// Non-blocking composite acquire. Returns `Ok(None)` immediately if any
    /// member key is contended; otherwise grabs all keys atomically.
    pub async fn try_acquire_composite(
        &self,
        keys: &[&str],
        ttl: Duration,
    ) -> Result<Option<LockGuard>, ClientError> {
        crate::routine_id!("ddl-routine-tryacq-comp-2Lm");
        if keys.is_empty() || keys.len() > MAX_COMPOSITE_KEYS {
            return Err(ClientError::Invalid(format!(
                "composite acquire requires 1..={MAX_COMPOSITE_KEYS} keys"
            )));
        }
        self.try_acquire_internal(
            None,
            Some(keys.iter().map(|s| s.to_string()).collect()),
            ttl,
            None,
        )
        .await
    }

    async fn try_acquire_internal(
        &self,
        key: Option<String>,
        keys: Option<Vec<String>>,
        ttl: Duration,
        max: Option<u32>,
    ) -> Result<Option<LockGuard>, ClientError> {
        crate::routine_id!("ddl-routine-tryacq-internal-9Vd");
        let request_uuid = Uuid::new_v4().to_string();
        let mut rx = self.register_inflight(&request_uuid);

        let request = Request::Lock {
            uuid: request_uuid.clone(),
            key,
            keys,
            pid: Some(std::process::id() as i64),
            ttl: Some(ttl.as_millis() as u64),
            max,
            force: false,
            retry_count: 0,
            keep_locks_after_death: false,
            wait: Some(false),
        };
        if let Err(err) = self.write_request(request).await {
            self.unregister_inflight(&request_uuid);
            return Err(err);
        }

        let timeout = self.inner.config.default_request_timeout;
        // No-wait: the broker sends exactly one terminal reply.
        let result = match self.roundtrip_recv(&mut rx, timeout).await {
            Ok(Response::Lock {
                acquired: true,
                key,
                lock_uuid: Some(lock_uuid),
                fencing_token,
                ..
            }) => Ok(Some(LockGuard::single(key, lock_uuid, fencing_token))),
            Ok(Response::CompositeLock {
                acquired: true,
                keys,
                lock_uuid: Some(lock_uuid),
                fencing_tokens,
                ..
            }) => Ok(Some(LockGuard::composite(
                keys,
                lock_uuid,
                fencing_tokens.unwrap_or_default(),
            ))),
            Ok(Response::Lock {
                acquired: false, ..
            })
            | Ok(Response::CompositeLock {
                acquired: false, ..
            }) => Ok(None),
            Ok(Response::Error { error, .. }) => Err(ClientError::Broker(error)),
            Ok(other) => Err(ClientError::Broker(format!(
                "unexpected try-acquire response: {other:?}"
            ))),
            Err(err) => Err(err),
        };
        self.unregister_inflight(&request_uuid);
        result
    }

    async fn roundtrip_recv(
        &self,
        rx: &mut mpsc::UnboundedReceiver<Response>,
        timeout: Duration,
    ) -> Result<Response, ClientError> {
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(resp)) => Ok(resp),
            Ok(None) => Err(ClientError::Closed),
            Err(_) => Err(ClientError::Timeout(timeout)),
        }
    }

    async fn wait_for_acquire(
        &self,
        rx: &mut mpsc::UnboundedReceiver<Response>,
        request_uuid: &str,
        timeout: Duration,
    ) -> Result<LockGuard, ClientError> {
        crate::routine_id!("ddl-routine-rYoW6DP7XrtX17pFyY");
        let deadline = deadline_after(timeout);
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
        crate::routine_id!("ddl-routine-NLencuC-xXTiVHvNsj");
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
            Response::Unlock { unlocked: true, .. } => Ok(()),
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
        crate::routine_id!("ddl-routine-JhVDA3885_Mh1B8gYL");
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
        crate::routine_id!("ddl-routine-AHMJ2XAm_T1ufxiHgH");
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
        crate::routine_id!("ddl-routine-2CBu_Ti9-v9H5eOGph");
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.inflight.lock().insert(uuid.to_string(), tx);
        rx
    }

    fn unregister_inflight(&self, uuid: &str) {
        crate::routine_id!("ddl-routine-xoZtjre8ORbuxPAw47");
        self.inner.inflight.lock().remove(uuid);
    }

    async fn write_request(&self, request: Request) -> Result<(), ClientError> {
        crate::routine_id!("ddl-routine-OJJ5gqugYULiWQ96ZX");
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
        crate::routine_id!("ddl-routine-UQewmszhGwoIxFb3Jn");
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
        crate::routine_id!("ddl-routine-Xw2i3L2CdWqt0hyUn8");
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
        crate::routine_id!("ddl-routine-BBsdaJ4ryHsNYPIk4P");
        Ok(Self {
            inner: Client::connect_tcp(addr, config).await?,
        })
    }

    pub async fn connect_uds(
        path: impl AsRef<Path>,
        config: ClientConfig,
    ) -> Result<Self, ClientError> {
        crate::routine_id!("ddl-routine-O6v0Ns6yFrkvxBM0gW");
        Ok(Self {
            inner: Client::connect_uds(path, config).await?,
        })
    }

    pub async fn acquire_read(&self, key: &str) -> Result<RwReadGuard, ClientError> {
        crate::routine_id!("ddl-routine-c_r-gmONuZMejrz47X");
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
        crate::routine_id!("ddl-routine-f32sihPjsPJC-vFo7O");
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
        crate::routine_id!("ddl-routine-_VkY3EWAQzWVwMceKa");
        let timeout = self.inner.inner.config.default_request_timeout;
        let deadline = deadline_after(timeout);
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
        crate::routine_id!("ddl-routine-43Vd0AnZsbqmAAi6eb");
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
        crate::routine_id!("ddl-routine-CTCD-uPtmZSyMSv2eo");
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
        crate::routine_id!("ddl-routine-Vjn5LJ94ZLnulDL8RZ");
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
        crate::routine_id!("ddl-routine-nRbRFq1_GRo4TWIU1y");
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
        crate::routine_id!("ddl-routine-PJgckbbW53tEIY21kv");
        &self.inner.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn response_frame_reader_preserves_split_utf8_and_trims_crlf() {
        let mut frame = serde_json::to_vec(&Response::Error {
            uuid: "u1".into(),
            error: "hello 😊".into(),
        })
        .unwrap();
        frame.extend_from_slice(b"\r\n");

        let emoji = "😊".as_bytes();
        let split = frame
            .windows(emoji.len())
            .position(|w| w == emoji)
            .expect("emoji bytes should be present")
            + 1;
        let first = frame[..split].to_vec();
        let second = frame[split..].to_vec();

        let (mut tx, rx) = tokio::io::duplex(256);
        let writer = tokio::spawn(async move {
            tx.write_all(&first).await.unwrap();
            tokio::task::yield_now().await;
            tx.write_all(&second).await.unwrap();
        });

        let mut reader = BufReader::new(rx);
        let mut buf = Vec::new();
        assert!(read_response_frame_bounded(&mut reader, &mut buf, 1024)
            .await
            .unwrap());
        writer.await.unwrap();

        let resp: Response = serde_json::from_slice(trim_response_frame(&buf)).unwrap();
        match resp {
            Response::Error { uuid, error } => {
                assert_eq!(uuid, "u1");
                assert_eq!(error, "hello 😊");
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[tokio::test]
    async fn response_frame_reader_rejects_oversized_unterminated_frame() {
        let (mut tx, rx) = tokio::io::duplex(16);
        let writer = tokio::spawn(async move {
            let _ = tx.write_all(&[b'x'; 128]).await;
        });

        let mut reader = BufReader::new(rx);
        let mut buf = Vec::new();
        let err = tokio::time::timeout(
            Duration::from_secs(1),
            read_response_frame_bounded(&mut reader, &mut buf, 32),
        )
        .await
        .expect("bounded reader should return promptly")
        .unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        writer.abort();
        let _ = writer.await;
    }

    #[tokio::test]
    async fn response_frame_reader_rejects_mid_frame_eof() {
        let (mut tx, rx) = tokio::io::duplex(64);
        tx.write_all(b"{\"type\":\"ok\"").await.unwrap();
        drop(tx);

        let mut reader = BufReader::new(rx);
        let mut buf = Vec::new();
        assert!(read_response_frame_bounded(&mut reader, &mut buf, 1024)
            .await
            .unwrap());

        assert!(serde_json::from_slice::<Response>(trim_response_frame(&buf)).is_err());
    }

    #[tokio::test]
    async fn response_frame_reader_accepts_final_response_without_newline() {
        let frame = serde_json::to_vec(&Response::Ok {
            uuid: "u-final".into(),
        })
        .unwrap();

        let (mut tx, rx) = tokio::io::duplex(128);
        tx.write_all(&frame).await.unwrap();
        drop(tx);

        let mut reader = BufReader::new(rx);
        let mut buf = Vec::new();
        assert!(read_response_frame_bounded(&mut reader, &mut buf, 1024)
            .await
            .unwrap());
        assert_eq!(trim_response_frame(&buf), frame.as_slice());

        let resp: Response = serde_json::from_slice(trim_response_frame(&buf)).unwrap();
        match resp {
            Response::Ok { uuid } => assert_eq!(uuid, "u-final"),
            other => panic!("unexpected response: {other:?}"),
        }
    }
}
