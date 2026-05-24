//! Transport listeners. The broker is transport-agnostic; this module wires it
//! up to:
//!
//! 1. **TCP** (`0.0.0.0:6970` by default) — newline-delimited JSON, one
//!    persistent connection per `Client`. Optional TLS via `rustls` when
//!    compiled with the `tls` feature and `LMX_TLS_CERT` / `LMX_TLS_KEY` are
//!    set. In production the load balancer is usually a better TLS terminator.
//! 2. **Unix Domain Socket** (`/tmp/dd-rust-network-mutex.sock` by default) —
//!    same wire format, no TLS, suitable for in-pod or in-host peers.
//! 3. **HTTP** (`0.0.0.0:6971` by default) — single-shot JSON-over-HTTP for
//!    serverless/Lambda callers. Long-poll support via `?wait_ms=` /
//!    `waitMs` body field.
//!
//! Auth: when `LMX_AUTH_TOKEN` is set, every TCP/UDS connection must send
//! `{"type":"auth","token":"..."}` first; HTTP callers must include
//! `Authorization: Bearer <token>` (or `X-LMX-Auth: <token>`).

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response as AxumResponse},
    routing::{get, post},
    Json, Router,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::broker::{Broker, BrokerConfig};
use crate::protocol::{
    http::{
        AcquireRequest, AcquireResponse, LockInfoResponse, ReleaseRequest, ReleaseResponse,
        RwAcquireRequest, RwAcquireResponse, RwReleaseRequest, RwReleaseResponse,
    },
    Request, Response,
};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub tcp_bind: Option<SocketAddr>,
    pub uds_path: Option<PathBuf>,
    pub http_bind: Option<SocketAddr>,
    pub auth_token: Option<String>,
    pub broker: BrokerConfig,
    /// Apply `TCP_NODELAY` on every accepted broker-side TCP socket. Default
    /// `true`. Disabling lets you A/B-test the socket-tuning experiment from
    /// upstream `live-mutex#22`.
    pub tcp_nodelay: bool,
    /// Apply `TCP_QUICKACK = 1` after every read on Linux. No-op on other
    /// platforms. Default `true`. The kernel option is one-shot, so we
    /// re-apply it inside the broker's read loop on every frame.
    pub tcp_quickack: bool,
    /// Optional dedicated HTML status page listener (upstream
    /// [`live-mutex#108`](https://github.com/ORESoftware/live-mutex/issues/108)).
    /// When `Some`, a small read-only HTTP server binds here and serves
    /// only operator views — `GET /`, `GET /status`, `GET /healthz`,
    /// `GET /metrics`. **No authentication is enforced** on this
    /// listener, so it should be bound to a private interface
    /// (loopback, VPN, or a NetworkPolicy-restricted port). The main
    /// `http_bind` API stays auth-gated.
    pub status_bind: Option<SocketAddr>,
    #[cfg(feature = "tls")]
    pub tls: Option<TlsConfig>,
}

#[cfg(feature = "tls")]
#[derive(Debug, Clone)]
pub struct TlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            tcp_bind: Some("0.0.0.0:6970".parse().unwrap()),
            uds_path: None,
            http_bind: Some("0.0.0.0:6971".parse().unwrap()),
            auth_token: None,
            broker: BrokerConfig::default(),
            tcp_nodelay: true,
            tcp_quickack: true,
            status_bind: None,
            #[cfg(feature = "tls")]
            tls: None,
        }
    }
}

#[derive(Clone)]
struct AppState {
    broker: Broker,
    auth_token: Option<String>,
    metrics: Arc<crate::metrics::Metrics>,
}

pub async fn run(config: ServerConfig) -> std::io::Result<()> {
    let broker = Broker::new(config.broker.clone());
    let metrics = Arc::new(crate::metrics::Metrics::new());
    let auth_token = config.auth_token.clone();

    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let mut listeners_bound = 0usize;

    // Single periodic TTL sweep instead of per-request timers — see
    // upstream `live-mutex#13`. The sweeper is owned by `tasks` so it
    // is cancelled together with the listeners on shutdown.
    tasks.push(broker.spawn_ttl_sweeper());

    if let Some(addr) = config.tcp_bind {
        let listener = TcpListener::bind(addr).await?;
        listeners_bound += 1;
        info!(
            target: "lmx::tcp",
            %addr,
            tcp_nodelay = config.tcp_nodelay,
            tcp_quickack = config.tcp_quickack && crate::sockopt::quickack_supported(),
            "listening",
        );
        let broker_c = broker.clone();
        let auth_c = auth_token.clone();
        let metrics_c = metrics.clone();
        let nodelay = config.tcp_nodelay;
        let quickack = config.tcp_quickack;
        #[cfg(feature = "tls")]
        let tls_acceptor = match config.tls.as_ref() {
            Some(t) => Some(build_tls_acceptor(t)?),
            None => None,
        };
        tasks.push(tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((sock, peer)) => {
                        debug!(target: "lmx::tcp", %peer, "accept");
                        metrics_c.tcp_connections_total.inc();
                        if nodelay && crate::sockopt::apply_nodelay(&sock).is_ok() {
                            metrics_c.tcp_nodelay_applied_total.inc();
                        }
                        // Snapshot the fd *before* `sock` is moved into a
                        // TLS wrapper. The fd lives as long as the
                        // connection (TLS owns the underlying socket).
                        let fd: std::os::fd::RawFd = {
                            use std::os::fd::AsRawFd;
                            sock.as_raw_fd()
                        };
                        let after_read = AfterRead::Tcp { fd, quickack };
                        let broker = broker_c.clone();
                        let auth = auth_c.clone();
                        let metrics_inner = metrics_c.clone();
                        #[cfg(feature = "tls")]
                        let tls_acceptor = tls_acceptor.clone();
                        tokio::spawn(async move {
                            #[cfg(feature = "tls")]
                            {
                                if let Some(acceptor) = tls_acceptor {
                                    match acceptor.accept(sock).await {
                                        Ok(stream) => {
                                            if let Err(err) = handle_stream(
                                                stream,
                                                broker,
                                                auth,
                                                metrics_inner,
                                                after_read,
                                            )
                                            .await
                                            {
                                                warn!(target: "lmx::tcp", %peer, error=%err, "client errored");
                                            }
                                        }
                                        Err(err) => {
                                            warn!(target: "lmx::tcp", %peer, error=%err, "tls handshake failed");
                                        }
                                    }
                                    return;
                                }
                            }
                            if let Err(err) = handle_stream(
                                sock,
                                broker,
                                auth,
                                metrics_inner,
                                after_read,
                            )
                            .await
                            {
                                warn!(target: "lmx::tcp", %peer, error=%err, "client errored");
                            }
                        });
                    }
                    Err(err) => {
                        error!(target: "lmx::tcp", error=%err, "accept failed");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }));
    }

    if let Some(path) = config.uds_path.clone() {
        let listener = UnixListener::bind(&path).map_err(|err| {
            std::io::Error::new(
                err.kind(),
                format!(
                    "failed to bind UDS at {}: {err} (is the path stale?)",
                    path.display()
                ),
            )
        })?;
        listeners_bound += 1;
        info!(target: "lmx::uds", path=%path.display(), "listening");
        let broker_c = broker.clone();
        let auth_c = auth_token.clone();
        let metrics_c = metrics.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((sock, _peer)) => {
                        metrics_c.uds_connections_total.inc();
                        let broker = broker_c.clone();
                        let auth = auth_c.clone();
                        let metrics_inner = metrics_c.clone();
                        tokio::spawn(async move {
                            if let Err(err) = handle_stream(
                                sock,
                                broker,
                                auth,
                                metrics_inner,
                                AfterRead::None,
                            )
                            .await
                            {
                                warn!(target: "lmx::uds", error=%err, "client errored");
                            }
                        });
                    }
                    Err(err) => {
                        error!(target: "lmx::uds", error=%err, "accept failed");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }));
    }

    let status_info = Arc::new(build_status_info(&config));

    if let Some(addr) = config.http_bind {
        let app_state = AppState {
            broker: broker.clone(),
            auth_token: auth_token.clone(),
            metrics: metrics.clone(),
        };
        let status_state = StatusAppState {
            broker: broker.clone(),
            metrics: metrics.clone(),
            info: status_info.clone(),
        };
        let app = Router::new()
            .route("/healthz", get(healthz))
            .route("/readyz", get(healthz))
            .route("/metrics", get(metrics_endpoint))
            .route("/v1/lock", post(http_acquire))
            .route("/v1/unlock", post(http_release))
            .route("/v1/rw/read", post(http_rw_read))
            .route("/v1/rw/read/end", post(http_rw_read_end))
            .route("/v1/rw/write", post(http_rw_write))
            .route("/v1/rw/write/end", post(http_rw_write_end))
            .route("/v1/lock-info/:key", get(http_lock_info))
            .route("/v1/locks", get(http_ls))
            .with_state(app_state)
            // Status views — `live-mutex#108`. Unauthenticated, matching
            // `/healthz` and `/metrics` on the same listener.
            .merge(
                Router::new()
                    .route("/", get(status_page))
                    .route("/status", get(status_page))
                    .with_state(status_state),
            );
        let listener = TcpListener::bind(addr).await?;
        listeners_bound += 1;
        info!(target: "lmx::http", %addr, "listening");
        tasks.push(tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, app).await {
                error!(target: "lmx::http", error=%err, "axum exited");
            }
        }));
    }

    // Optional dedicated status-only listener (upstream `live-mutex#108`).
    // Bind a *separate* port whose surface is strictly read-only, so an
    // operator can expose just the HTML view without exposing the API.
    if let Some(addr) = config.status_bind {
        let status_state = StatusAppState {
            broker: broker.clone(),
            metrics: metrics.clone(),
            info: status_info.clone(),
        };
        let app = status_router(status_state);
        let listener = TcpListener::bind(addr).await?;
        listeners_bound += 1;
        info!(
            target: "lmx::status",
            %addr,
            "status page listening (no auth — bind to a private interface)",
        );
        tasks.push(tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, app).await {
                error!(target: "lmx::status", error=%err, "axum exited");
            }
        }));
    }

    if listeners_bound == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "no listeners configured (set at least one of tcp_bind, uds_path, http_bind, status_bind)",
        ));
    }

    let (_first, _idx, _rest) = futures_util::future::select_all(tasks).await;
    Ok(())
}

#[cfg(feature = "tls")]
fn build_tls_acceptor(cfg: &TlsConfig) -> Result<tokio_rustls::TlsAcceptor, std::io::Error> {
    use std::fs::File;
    use std::io::BufReader as StdBufReader;
    let cert_file = File::open(&cfg.cert_path)?;
    let key_file = File::open(&cfg.key_path)?;
    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut StdBufReader::new(cert_file))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let key = rustls_pemfile::private_key(&mut StdBufReader::new(key_file))
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no private key"))?;
    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(server_config)))
}

/// Per-connection hook that runs after every successful frame read. Used
/// to re-apply `TCP_QUICKACK` on Linux (the option is one-shot).
#[derive(Clone, Copy)]
pub(crate) enum AfterRead {
    /// Non-TCP transport (UDS, in-process tests). No-op.
    None,
    /// TCP connection. We hold the raw fd; on Linux we set TCP_QUICKACK
    /// after every read so the kernel ACKs immediately rather than
    /// queueing a delayed ACK.
    Tcp {
        fd: std::os::fd::RawFd,
        quickack: bool,
    },
}

impl AfterRead {
    fn run(&self, metrics: &crate::metrics::Metrics) {
        match self {
            AfterRead::None => {}
            AfterRead::Tcp { fd, quickack } => {
                if *quickack {
                    if let Ok(true) = crate::sockopt::apply_quickack(*fd) {
                        metrics.tcp_quickack_applied_total.inc();
                    }
                }
            }
        }
    }
}

async fn handle_stream<S>(
    stream: S,
    broker: Broker,
    auth_token: Option<String>,
    metrics: Arc<crate::metrics::Metrics>,
    after_read: AfterRead,
) -> std::io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
    let (client_id, mut rx) = broker.register_client();
    let mut buf = String::new();
    let mut reader = BufReader::new(read);
    let mut authed = auth_token.is_none();

    let writer_task = tokio::spawn(async move {
        while let Some(resp) = rx.recv().await {
            match serde_json::to_vec(&resp) {
                Ok(mut bytes) => {
                    bytes.push(b'\n');
                    if write.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Err(err) => {
                    error!(target: "lmx::tcp", error=%err, "serialize response");
                }
            }
        }
        let _ = write.shutdown().await;
    });

    let result = async {
        loop {
            buf.clear();
            let n = reader.read_line(&mut buf).await?;
            if n == 0 {
                break;
            }
            // Re-apply TCP_QUICKACK *immediately* after we've consumed a
            // frame from the kernel. This wins back the ~40 ms delayed-ACK
            // penalty on Linux for the next inbound segment. See
            // upstream issue ORESoftware/live-mutex#22 and
            // src/sockopt.rs.
            after_read.run(&metrics);
            let line = buf.trim();
            if line.is_empty() {
                continue;
            }
            metrics.requests_total.inc();
            let request: Request = match serde_json::from_str(line) {
                Ok(r) => r,
                Err(err) => {
                    metrics.malformed_requests_total.inc();
                    broker.try_send(
                        client_id,
                        Response::Error {
                            uuid: "malformed".into(),
                            error: format!("malformed request: {err}"),
                        },
                    );
                    continue;
                }
            };
            if !authed {
                if let Request::Auth { uuid, token } = &request {
                    if Some(token) == auth_token.as_ref() {
                        authed = true;
                        broker.try_send(
                            client_id,
                            Response::Auth {
                                uuid: uuid.clone(),
                                ok: true,
                                error: None,
                            },
                        );
                        continue;
                    }
                    metrics.auth_failures_total.inc();
                    broker.try_send(
                        client_id,
                        Response::Auth {
                            uuid: uuid.clone(),
                            ok: false,
                            error: Some("invalid auth token".into()),
                        },
                    );
                    break;
                }
                metrics.auth_failures_total.inc();
                broker.try_send(
                    client_id,
                    Response::Error {
                        uuid: "unauth".into(),
                        error: "auth handshake required".into(),
                    },
                );
                break;
            }
            broker.handle_request(client_id, request);
        }
        Ok::<(), std::io::Error>(())
    }
    .await;

    broker.drop_client(client_id);
    drop(reader);
    let _ = writer_task.await;
    result
}

#[allow(dead_code)]
async fn _ensure_tcp_handler_compiles(
    sock: TcpStream,
    broker: Broker,
    auth_token: Option<String>,
    metrics: Arc<crate::metrics::Metrics>,
) -> std::io::Result<()> {
    handle_stream(sock, broker, auth_token, metrics, AfterRead::None).await
}

#[allow(dead_code)]
async fn _ensure_uds_handler_compiles(
    sock: UnixStream,
    broker: Broker,
    auth_token: Option<String>,
    metrics: Arc<crate::metrics::Metrics>,
) -> std::io::Result<()> {
    handle_stream(sock, broker, auth_token, metrics, AfterRead::None).await
}

// ---------------- HTTP layer -----------------------------------------------

fn http_unauthorized() -> AxumResponse {
    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
}

fn http_authorized(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(token) = state.auth_token.as_ref() else {
        return true;
    };
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    let custom = headers
        .get("x-lmx-auth")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    bearer.as_deref() == Some(token.as_str()) || custom.as_deref() == Some(token.as_str())
}

async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({"ok": true, "service": "dd-rust-network-mutex"}))
}

async fn metrics_endpoint(State(state): State<AppState>) -> impl IntoResponse {
    let body = state.metrics.render(&state.broker);
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        body,
    )
}

/// HTML status page — upstream `live-mutex#108`.
///
/// Available on both the main HTTP listener (so a single port suffices
/// for an operator who wants the page) and the optional dedicated
/// status listener (`status_bind`). The handler is read-only and
/// intentionally does not require authentication, matching `/healthz`
/// and `/metrics`. Operators relying on private posture should bind
/// `status_bind` to loopback or a VPN-only interface.
async fn status_page(State(state): State<StatusAppState>) -> impl IntoResponse {
    let metrics_text = state.metrics.render(&state.broker);
    let html = crate::status::render(&state.broker, &state.info, &metrics_text);
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
}

/// State for the status routes. Distinct from `AppState` so the
/// dedicated status listener doesn't accidentally pull in the
/// auth-token field.
#[derive(Clone)]
struct StatusAppState {
    broker: Broker,
    metrics: Arc<crate::metrics::Metrics>,
    info: Arc<crate::status::StatusServerInfo>,
}

fn build_status_info(config: &ServerConfig) -> crate::status::StatusServerInfo {
    crate::status::StatusServerInfo {
        tcp_bind: config.tcp_bind.map(|a| a.to_string()),
        uds_path: config
            .uds_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        http_bind: config.http_bind.map(|a| a.to_string()),
        status_bind: config.status_bind.map(|a| a.to_string()),
        auth_token_set: config.auth_token.is_some(),
        tcp_nodelay: config.tcp_nodelay,
        tcp_quickack: config.tcp_quickack,
        tcp_quickack_effective: config.tcp_quickack && crate::sockopt::quickack_supported(),
        default_ttl: config.broker.default_ttl,
        ttl_sweep_interval: config.broker.ttl_sweep_interval,
        max_lock_holders: config.broker.max_lock_holders,
        max_concurrency_cap: config.broker.max_concurrency_cap,
        #[cfg(feature = "tls")]
        tls_enabled: config.tls.is_some(),
        #[cfg(not(feature = "tls"))]
        tls_enabled: false,
    }
}

fn status_router(state: StatusAppState) -> Router {
    Router::new()
        .route("/", get(status_page))
        .route("/status", get(status_page))
        .route("/healthz", get(healthz))
        .route("/readyz", get(healthz))
        .route("/metrics", get(metrics_endpoint_status))
        .with_state(state)
}

async fn metrics_endpoint_status(State(state): State<StatusAppState>) -> impl IntoResponse {
    let body = state.metrics.render(&state.broker);
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        body,
    )
}

/// Run a single broker request in an ephemeral client and await its response.
/// The lock-acquisition happy path detaches the resulting `lock_uuid` from the
/// ephemeral client so the caller can release it later via /v1/unlock without
/// relying on a long-lived connection.
async fn run_ephemeral(
    state: &AppState,
    request: Request,
    request_uuid: &str,
    wait: Duration,
    is_acquire: bool,
) -> Option<Response> {
    let (client_id, mut rx) = state.broker.register_client();
    state.broker.handle_request(client_id, request);
    let outcome = wait_for(&mut rx, request_uuid, wait, is_acquire).await;
    if let Some(ref resp) = outcome {
        let granted_uuid = match resp {
            Response::Lock {
                acquired: true,
                lock_uuid: Some(u),
                ..
            } => Some(u.clone()),
            Response::CompositeLock {
                acquired: true,
                lock_uuid: Some(u),
                ..
            } => Some(u.clone()),
            Response::RegisterReadResult {
                granted: true,
                lock_uuid: Some(u),
                ..
            } => Some(u.clone()),
            Response::RegisterWriteResult {
                granted: true,
                lock_uuid: Some(u),
                ..
            } => Some(u.clone()),
            _ => None,
        };
        if let Some(lock_uuid) = granted_uuid {
            // Detach so the broker doesn't release the lock when we drop the
            // ephemeral client below. Subsequent /v1/unlock matches by uuid.
            state.broker.detach_lock_from_client(client_id, &lock_uuid);
        }
    }
    state.broker.drop_client(client_id);
    let _ = rx;
    outcome
}

async fn http_acquire(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AcquireRequest>,
) -> AxumResponse {
    if !http_authorized(&state, &headers) {
        return http_unauthorized();
    }
    state.metrics.requests_total.inc();
    let request_uuid = Uuid::new_v4().to_string();
    let request = Request::Lock {
        uuid: request_uuid.clone(),
        key: req.key.clone(),
        keys: req.keys.clone(),
        pid: None,
        ttl: req.ttl_ms,
        max: req.max,
        force: false,
        retry_count: 0,
        keep_locks_after_death: false,
    };
    let wait = req.wait_ms.map(Duration::from_millis).unwrap_or_default();
    let outcome = run_ephemeral(&state, request, &request_uuid, wait, true).await;
    match outcome {
        Some(Response::Lock {
            acquired,
            key,
            lock_uuid,
            fencing_token,
            lock_request_count,
            error,
            ..
        }) => {
            let mut tokens = BTreeMap::new();
            if let Some(t) = fencing_token {
                tokens.insert(key.clone(), t);
            }
            let body = AcquireResponse {
                acquired,
                keys: vec![key],
                lock_uuid,
                fencing_tokens: tokens,
                queue_depth: lock_request_count,
                error: error.clone(),
            };
            // Synchronously-rejected requests (validation errors,
            // mutually-exclusive `key`+`keys`, etc.) come back as
            // `Lock { acquired: false, error: Some(_) }`. Surface
            // those as 400, not as 200 with `acquired:false` —
            // otherwise misconfigured callers silently sit on a
            // request that will never grant.
            if !acquired && body.error.is_some() {
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            } else {
                Json(body).into_response()
            }
        }
        Some(Response::CompositeLock {
            acquired,
            keys,
            lock_uuid,
            fencing_tokens,
            error,
            ..
        }) => {
            let body = AcquireResponse {
                acquired,
                keys,
                lock_uuid,
                fencing_tokens: fencing_tokens.unwrap_or_default(),
                queue_depth: 0,
                error: error.clone(),
            };
            if !acquired && body.error.is_some() {
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            } else {
                Json(body).into_response()
            }
        }
        Some(Response::Error { error, .. }) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"acquired": false, "error": error})),
        )
            .into_response(),
        _ => Json(AcquireResponse {
            acquired: false,
            keys: req.keys.unwrap_or_else(|| req.key.into_iter().collect()),
            lock_uuid: None,
            fencing_tokens: BTreeMap::new(),
            queue_depth: 0,
            error: None,
        })
        .into_response(),
    }
}

async fn http_release(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ReleaseRequest>,
) -> AxumResponse {
    if !http_authorized(&state, &headers) {
        return http_unauthorized();
    }
    state.metrics.requests_total.inc();
    let request_uuid = Uuid::new_v4().to_string();
    let outcome = run_ephemeral(
        &state,
        Request::Unlock {
            uuid: request_uuid.clone(),
            key: req.key.clone(),
            keys: req.keys.clone(),
            lock_uuid: req.lock_uuid.clone(),
            force: req.force,
        },
        &request_uuid,
        Duration::from_millis(2000),
        false,
    )
    .await;
    match outcome {
        Some(Response::Unlock {
            keys, unlocked, ..
        }) => Json(ReleaseResponse { unlocked, keys }).into_response(),
        Some(Response::Error { error, .. }) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"unlocked": false, "error": error})),
        )
            .into_response(),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"unlocked": false, "error": "broker timed out"})),
        )
            .into_response(),
    }
}

async fn http_rw_read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RwAcquireRequest>,
) -> AxumResponse {
    if !http_authorized(&state, &headers) {
        return http_unauthorized();
    }
    state.metrics.requests_total.inc();
    let request_uuid = Uuid::new_v4().to_string();
    let wait = req.wait_ms.map(Duration::from_millis).unwrap_or_default();
    let outcome = run_ephemeral(
        &state,
        Request::RegisterRead {
            uuid: request_uuid.clone(),
            key: req.key.clone(),
        },
        &request_uuid,
        wait,
        true,
    )
    .await;
    match outcome {
        Some(Response::RegisterReadResult {
            granted,
            key,
            readers_count,
            writer_flag,
            lock_uuid,
            fencing_token,
            ..
        }) => Json(RwAcquireResponse {
            granted,
            key,
            readers_count,
            writer_flag,
            lock_uuid,
            fencing_token,
        })
        .into_response(),
        _ => Json(RwAcquireResponse {
            granted: false,
            key: req.key,
            readers_count: 0,
            writer_flag: false,
            lock_uuid: None,
            fencing_token: None,
        })
        .into_response(),
    }
}

async fn http_rw_read_end(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RwReleaseRequest>,
) -> AxumResponse {
    if !http_authorized(&state, &headers) {
        return http_unauthorized();
    }
    state.metrics.requests_total.inc();
    let request_uuid = Uuid::new_v4().to_string();
    let outcome = run_ephemeral(
        &state,
        Request::Unlock {
            uuid: request_uuid.clone(),
            key: Some(req.key.clone()),
            keys: None,
            lock_uuid: Some(req.lock_uuid.clone()),
            force: false,
        },
        &request_uuid,
        Duration::from_millis(2000),
        false,
    )
    .await;
    match outcome {
        Some(Response::Unlock { unlocked, .. }) if unlocked => Json(RwReleaseResponse {
            key: req.key,
            readers_count: 0,
            writer_flag: false,
        })
        .into_response(),
        _ => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"unlocked": false})),
        )
            .into_response(),
    }
}

async fn http_rw_write(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RwAcquireRequest>,
) -> AxumResponse {
    if !http_authorized(&state, &headers) {
        return http_unauthorized();
    }
    state.metrics.requests_total.inc();
    let request_uuid = Uuid::new_v4().to_string();
    let wait = req.wait_ms.map(Duration::from_millis).unwrap_or_default();
    let outcome = run_ephemeral(
        &state,
        Request::RegisterWrite {
            uuid: request_uuid.clone(),
            key: req.key.clone(),
        },
        &request_uuid,
        wait,
        true,
    )
    .await;
    match outcome {
        Some(Response::RegisterWriteResult {
            granted,
            key,
            readers_count,
            writer_flag,
            lock_uuid,
            fencing_token,
            ..
        }) => Json(RwAcquireResponse {
            granted,
            key,
            readers_count,
            writer_flag,
            lock_uuid,
            fencing_token,
        })
        .into_response(),
        _ => Json(RwAcquireResponse {
            granted: false,
            key: req.key,
            readers_count: 0,
            writer_flag: false,
            lock_uuid: None,
            fencing_token: None,
        })
        .into_response(),
    }
}

async fn http_rw_write_end(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RwReleaseRequest>,
) -> AxumResponse {
    http_rw_read_end(State(state), headers, Json(req)).await
}

async fn http_lock_info(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> AxumResponse {
    if !http_authorized(&state, &headers) {
        return http_unauthorized();
    }
    state.metrics.requests_total.inc();
    let request_uuid = Uuid::new_v4().to_string();
    let outcome = run_ephemeral(
        &state,
        Request::LockInfo {
            uuid: request_uuid.clone(),
            key: key.clone(),
        },
        &request_uuid,
        Duration::from_millis(500),
        false,
    )
    .await;
    match outcome {
        Some(Response::LockInfo {
            is_locked,
            lockholder_uuids,
            lock_request_count,
            readers_count,
            writer_flag,
            ..
        }) => Json(LockInfoResponse {
            key,
            is_locked,
            lockholder_uuids,
            lock_request_count,
            readers_count,
            writer_flag,
        })
        .into_response(),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "broker did not respond").into_response(),
    }
}

async fn http_ls(State(state): State<AppState>, headers: HeaderMap) -> AxumResponse {
    if !http_authorized(&state, &headers) {
        return http_unauthorized();
    }
    state.metrics.requests_total.inc();
    let request_uuid = Uuid::new_v4().to_string();
    let outcome = run_ephemeral(
        &state,
        Request::Ls {
            uuid: request_uuid.clone(),
        },
        &request_uuid,
        Duration::from_millis(500),
        false,
    )
    .await;
    match outcome {
        Some(Response::LsResult { keys, .. }) => {
            Json(serde_json::json!({"keys": keys})).into_response()
        }
        _ => (StatusCode::SERVICE_UNAVAILABLE, "broker did not respond").into_response(),
    }
}

async fn wait_for(
    rx: &mut mpsc::UnboundedReceiver<Response>,
    request_uuid: &str,
    wait: Duration,
    keep_polling_until_definitive: bool,
) -> Option<Response> {
    let _ = keep_polling_until_definitive;
    let timeout = if wait.is_zero() {
        Duration::from_millis(50)
    } else {
        wait
    };
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last_match: Option<Response> = None;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let res = if remaining.is_zero() {
            rx.try_recv().ok()
        } else {
            tokio::time::timeout(remaining, rx.recv())
                .await
                .ok()
                .flatten()
        };
        match res {
            Some(msg) if msg.correlation_uuid() == request_uuid => {
                let definitive = matches!(
                    &msg,
                    Response::Lock { acquired: true, .. }
                        | Response::CompositeLock { acquired: true, .. }
                        | Response::RegisterReadResult { granted: true, .. }
                        | Response::RegisterWriteResult { granted: true, .. }
                        | Response::Unlock { .. }
                        | Response::EndReadResult { .. }
                        | Response::EndWriteResult { .. }
                        | Response::LockInfo { .. }
                        | Response::LsResult { .. }
                        | Response::Error { .. }
                );
                last_match = Some(msg);
                if definitive {
                    return last_match;
                }
                if !keep_polling_until_definitive {
                    return last_match;
                }
            }
            Some(_) => continue,
            None => return last_match,
        }
    }
}

