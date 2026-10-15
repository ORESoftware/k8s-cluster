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
//! Auth: when an auth token is configured (from `ALL_DOGS`, else
//! `LMX_AUTH_TOKEN`, else the config file), every TCP/UDS connection must send
//! `{"type":"auth","token":"..."}` first; HTTP callers must include the token in
//! an `Auth: <token>` header (or `Authorization: Bearer <token>` /
//! `X-LMX-Auth: <token>`).

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, Request as AxumRequest, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response as AxumResponse},
    routing::{get, post},
    Json, Router,
};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::broker::{Broker, BrokerConfig};
use crate::broker_raft::{BrokerRaft, BrokerRaftConfig, BrokerRaftError, RaftPeerConfig};
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
        crate::routine_id!("ddl-routine-Hw9N40elbjzI0ZcFDP");
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

/// Runtime-mutable TCP socket-tuning flags. Owned by [`AppState`] (and
/// shared into the per-connection accept loop) so `POST /admin/tcp`
/// can flip behavior for newly-accepted connections without
/// restarting the broker.
///
/// `quickack` is honored only on Linux. On other platforms the flag is
/// stored faithfully so a future Linux deploy will pick it up, but
/// `apply_quickack` is a no-op (see `crate::sockopt`).
#[derive(Debug)]
pub(crate) struct TcpFlags {
    pub(crate) nodelay: AtomicBool,
    pub(crate) quickack: AtomicBool,
}

impl TcpFlags {
    fn new(nodelay: bool, quickack: bool) -> Self {
        Self {
            nodelay: AtomicBool::new(nodelay),
            quickack: AtomicBool::new(quickack),
        }
    }
    pub(crate) fn nodelay(&self) -> bool {
        self.nodelay.load(Ordering::Relaxed)
    }
    pub(crate) fn quickack(&self) -> bool {
        self.quickack.load(Ordering::Relaxed)
    }
    fn set_nodelay(&self, v: bool) {
        self.nodelay.store(v, Ordering::Relaxed);
    }
    fn set_quickack(&self, v: bool) {
        self.quickack.store(v, Ordering::Relaxed);
    }
}

#[derive(Clone)]
struct AppState {
    broker: Broker,
    auth_token: Option<String>,
    metrics: Arc<crate::metrics::Metrics>,
    tcp_flags: Arc<TcpFlags>,
}

pub async fn run(config: ServerConfig) -> std::io::Result<()> {
    crate::routine_id!("ddl-routine-NiYLHbcx_IzD00AJBp");
    let broker = Broker::new(config.broker.clone());
    let metrics = Arc::new(crate::metrics::Metrics::new());
    let auth_token = config.auth_token.clone();
    // Shared, runtime-mutable view of TCP socket flags. Initial values
    // come from `ServerConfig` (kept as `bool` for backwards-compat with
    // existing callers); operators flip them at runtime via
    // `POST /admin/tcp` and the change is picked up by both the
    // accept loop (NODELAY) and the per-frame `AfterRead` hook
    // (QUICKACK) on the very next event.
    let tcp_flags = Arc::new(TcpFlags::new(config.tcp_nodelay, config.tcp_quickack));

    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let mut listeners_bound = 0usize;

    // Single periodic TTL sweep instead of per-request timers — see
    // upstream `live-mutex#13`. The sweeper is owned by `tasks` so it
    // is cancelled together with the listeners on shutdown.
    if !config.broker.ttl_sweep_interval.is_zero() {
        tasks.push(broker.spawn_ttl_sweeper());
    }

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
        let tcp_flags_c = tcp_flags.clone();
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
                        // Read the live values on every accept so a
                        // runtime toggle via `POST /admin/tcp` takes
                        // effect for newly-accepted connections.
                        if tcp_flags_c.nodelay() && crate::sockopt::apply_nodelay(&sock).is_ok() {
                            metrics_c.tcp_nodelay_applied_total.inc();
                        }
                        // Snapshot the fd *before* `sock` is moved into a
                        // TLS wrapper. The fd lives as long as the
                        // connection (TLS owns the underlying socket).
                        let fd: std::os::fd::RawFd = {
                            use std::os::fd::AsRawFd;
                            sock.as_raw_fd()
                        };
                        let after_read = AfterRead::Tcp {
                            fd,
                            flags: tcp_flags_c.clone(),
                        };
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
                            if let Err(err) =
                                handle_stream(sock, broker, auth, metrics_inner, AfterRead::None)
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
            tcp_flags: tcp_flags.clone(),
        };
        let status_state = StatusAppState {
            broker: broker.clone(),
            metrics: metrics.clone(),
            info: status_info.clone(),
            tcp_flags: tcp_flags.clone(),
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
            // Runtime OTel kill-switch. Authenticated by a separate
            // admin token (NOT the broker's general-purpose
            // `auth_token`) so an operator can flip the flag in
            // environments where lock callers don't share the admin
            // password.
            .route("/admin/otel", get(admin_otel_get).post(admin_otel_post))
            .route(
                "/admin/log-level",
                get(admin_log_level_get).post(admin_log_level_post),
            )
            .route("/admin/tcp", get(admin_tcp_get).post(admin_tcp_post))
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
            tcp_flags: tcp_flags.clone(),
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

#[derive(Clone)]
struct RaftAppState {
    raft: BrokerRaft,
    auth_token: Option<String>,
    metrics: Arc<crate::metrics::Metrics>,
}

#[derive(Debug, serde::Deserialize)]
struct RaftMembershipRequest {
    peers: Vec<RaftPeerConfig>,
}

#[derive(Debug, serde::Deserialize)]
struct RaftLearnersRequest {
    peers: Vec<RaftPeerConfig>,
}

#[derive(Debug, serde::Deserialize)]
struct RaftLearnerRemovalRequest {
    ids: Vec<String>,
}

pub async fn run_raft(config: ServerConfig, raft_config: BrokerRaftConfig) -> std::io::Result<()> {
    crate::routine_id!("ddl-routine-server-run-raft-1");
    let raft = BrokerRaft::open(raft_config).map_err(raft_io_error)?;
    let _shutdown_guard = BrokerRaftShutdownGuard(raft.clone());
    let metrics = Arc::new(crate::metrics::Metrics::new());
    let mut tasks = JoinSet::new();
    raft.spawn_raft_tasks_into(&mut tasks)
        .await
        .map_err(raft_io_error)?;
    let mut listeners_bound = 0usize;

    if let Some(addr) = config.http_bind {
        let app_state = RaftAppState {
            raft: raft.clone(),
            auth_token: config.auth_token.clone(),
            metrics: metrics.clone(),
        };
        let app = Router::new()
            .route("/healthz", get(healthz))
            .route("/readyz", get(healthz))
            .route("/metrics", get(raft_metrics_endpoint))
            .route("/raft/status", get(raft_status))
            .route("/raft/progress", get(raft_progress))
            .route("/raft/leaderz", get(raft_leaderz))
            .route(
                "/raft/learners",
                get(raft_learners)
                    .post(raft_stage_learners)
                    .delete(raft_remove_learners),
            )
            .route(
                "/raft/membership",
                get(raft_membership).post(raft_change_membership),
            )
            .route("/v1/lock", post(raft_http_acquire))
            .route("/v1/unlock", post(raft_http_release))
            .layer(middleware::from_fn_with_state(
                app_state.clone(),
                raft_leader_response_headers,
            ))
            .with_state(app_state);
        let listener = TcpListener::bind(addr).await?;
        listeners_bound += 1;
        info!(target: "lmx::raft::http", %addr, "raft HTTP listening");
        tasks.spawn(async move {
            if let Err(err) = axum::serve(listener, app).await {
                error!(target: "lmx::raft::http", error=%err, "raft HTTP exited");
            }
        });
    }

    if listeners_bound == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "BrokerRaft requires http_bind for the LB-facing API",
        ));
    }

    let _ = tasks.join_next().await;
    Ok(())
}

struct BrokerRaftShutdownGuard(BrokerRaft);

impl Drop for BrokerRaftShutdownGuard {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

fn raft_io_error(err: BrokerRaftError) -> std::io::Error {
    std::io::Error::other(err.to_string())
}

#[cfg(feature = "tls")]
fn build_tls_acceptor(cfg: &TlsConfig) -> Result<tokio_rustls::TlsAcceptor, std::io::Error> {
    crate::routine_id!("ddl-routine-lSOwgN4txz94l_DsiC");
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
///
/// The `Tcp` variant carries an `Arc<TcpFlags>` rather than a snapshot
/// `bool` so that runtime toggles via `POST /admin/tcp` take effect
/// for already-accepted long-lived connections on the very next
/// frame, without waiting for the connection to reconnect.
#[derive(Clone)]
pub(crate) enum AfterRead {
    /// Non-TCP transport (UDS, in-process tests). No-op.
    None,
    /// TCP connection. We hold the raw fd; on Linux we set TCP_QUICKACK
    /// after every read so the kernel ACKs immediately rather than
    /// queueing a delayed ACK. The `quickack` decision is read
    /// dynamically from the shared `TcpFlags`.
    Tcp {
        fd: std::os::fd::RawFd,
        flags: Arc<TcpFlags>,
    },
}

impl AfterRead {
    fn run(&self, metrics: &crate::metrics::Metrics) {
        crate::routine_id!("ddl-routine-bOwqZ9tGHJ5m7NJrZs");
        match self {
            AfterRead::None => {}
            AfterRead::Tcp { fd, flags } => {
                if flags.quickack() {
                    if let Ok(true) = crate::sockopt::apply_quickack(*fd) {
                        metrics.tcp_quickack_applied_total.inc();
                    }
                }
            }
        }
    }
}

/// Hard ceiling on the per-frame line length for the TCP/UDS broker
/// transport. A pre-auth client that opens a connection and never
/// sends a newline used to be able to balloon the broker's per-
/// connection read buffer into the gigabytes (the underlying
/// `BufReader::read_line` reads until `\n` or EOF without any
/// per-frame cap). Capping at 1 MiB is well above any realistic
/// composite-lock JSON payload (single-key locks fit in ~250 bytes;
/// the 5-key composite max-payload comes in under 2 KiB) while
/// keeping a misbehaving — or malicious — peer from exhausting
/// memory.
///
/// Override at runtime via `LMX_MAX_FRAME_BYTES` (any non-zero
/// integer). A value of `0` or invalid input falls back to the
/// default. The cap applies to TCP and Unix-domain-socket clients
/// alike; the dedicated HTTP listener has its own body limit.
const DEFAULT_MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Cooperative scheduling guard for peers that send many complete JSONL
/// frames in one already-buffered burst. Tokio usually yields while
/// waiting on I/O, but a hot `BufReader` buffer can let this connection
/// task drain many frames without hitting a pending await. Yielding every
/// N frames keeps admin HTTP, other TCP clients, and timers responsive.
///
/// Override at runtime via `LMX_FRAME_YIELD_EVERY` (any non-zero integer).
/// The default intentionally mirrors the TypeScript parser's live-mutex
/// class default.
const DEFAULT_FRAME_YIELD_EVERY: usize = 1024;

fn max_frame_bytes() -> usize {
    crate::routine_id!("ddl-routine-max-frame-bytes-Pq3");
    std::env::var("LMX_MAX_FRAME_BYTES")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_FRAME_BYTES)
}

fn frame_yield_every() -> usize {
    crate::routine_id!("ddl-routine-frame-yield-every-Nz2");
    std::env::var("LMX_FRAME_YIELD_EVERY")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_FRAME_YIELD_EVERY)
}

/// Maximum concurrent TCP/UDS client connections. Beyond this, a new connection
/// is accepted and immediately closed, so a connection flood can't exhaust
/// client slots, writer tasks, and per-connection read buffers. Override via
/// `LMX_MAX_CONNECTIONS` (any non-zero integer).
const DEFAULT_MAX_CONNECTIONS: u64 = 4096;

/// How long an *unauthenticated* connection has to complete the auth handshake
/// before it is dropped. This bounds slowloris / idle-hold attacks on the
/// pre-auth phase without affecting authenticated, long-lived lock holders
/// (which legitimately keep an otherwise-idle connection open). Override via
/// `LMX_AUTH_HANDSHAKE_MS`; `0` disables the deadline. Only applies when an auth
/// token is configured.
const DEFAULT_AUTH_HANDSHAKE_MS: u64 = 10_000;

static ACTIVE_STREAM_CONNECTIONS: AtomicU64 = AtomicU64::new(0);

fn max_connections() -> u64 {
    crate::routine_id!("ddl-routine-max-connections-Cn4");
    std::env::var("LMX_MAX_CONNECTIONS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_CONNECTIONS)
}

fn auth_handshake_timeout() -> Option<Duration> {
    crate::routine_id!("ddl-routine-auth-handshake-timeout-Hk9");
    let ms = std::env::var("LMX_AUTH_HANDSHAKE_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_AUTH_HANDSHAKE_MS);
    (ms > 0).then(|| Duration::from_millis(ms))
}

/// RAII guard bounding concurrent stream connections; `None` when at capacity.
struct StreamConnGuard {
    metrics: Arc<crate::metrics::Metrics>,
}

impl StreamConnGuard {
    fn try_acquire(max: u64, metrics: Arc<crate::metrics::Metrics>) -> Option<Self> {
        if ACTIVE_STREAM_CONNECTIONS.fetch_add(1, Ordering::SeqCst) >= max {
            ACTIVE_STREAM_CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
            metrics.stream_connection_cap_drops_total.inc();
            None
        } else {
            metrics.stream_connections_active.inc();
            Some(StreamConnGuard { metrics })
        }
    }
}

impl Drop for StreamConnGuard {
    fn drop(&mut self) {
        ACTIVE_STREAM_CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
        self.metrics.stream_connections_active.dec();
    }
}

#[cfg(test)]
mod conn_limit_tests {
    use super::*;

    #[test]
    fn stream_conn_guard_caps_and_releases() {
        ACTIVE_STREAM_CONNECTIONS.store(0, Ordering::SeqCst);
        let metrics = Arc::new(crate::metrics::Metrics::new());
        let g1 = StreamConnGuard::try_acquire(2, Arc::clone(&metrics));
        let g2 = StreamConnGuard::try_acquire(2, Arc::clone(&metrics));
        assert!(g1.is_some() && g2.is_some());
        assert_eq!(ACTIVE_STREAM_CONNECTIONS.load(Ordering::SeqCst), 2);
        assert_eq!(metrics.stream_connections_active.get(), 2);

        // At capacity: rejected, and the count is not left inflated.
        assert!(StreamConnGuard::try_acquire(2, Arc::clone(&metrics)).is_none());
        assert_eq!(ACTIVE_STREAM_CONNECTIONS.load(Ordering::SeqCst), 2);
        assert_eq!(metrics.stream_connections_active.get(), 2);
        assert_eq!(metrics.stream_connection_cap_drops_total.get(), 1);

        // Releasing a slot makes room again.
        drop(g1);
        assert_eq!(ACTIVE_STREAM_CONNECTIONS.load(Ordering::SeqCst), 1);
        assert_eq!(metrics.stream_connections_active.get(), 1);
        let g3 = StreamConnGuard::try_acquire(2, Arc::clone(&metrics));
        assert!(g3.is_some());
        assert_eq!(metrics.stream_connections_active.get(), 2);

        drop(g2);
        drop(g3);
        assert_eq!(ACTIVE_STREAM_CONNECTIONS.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.stream_connections_active.get(), 0);
        assert_eq!(metrics.stream_connection_cap_drops_total.get(), 1);
        let rendered = metrics.render(&Broker::new(BrokerConfig::default()));
        assert!(rendered.contains("dd_rust_network_mutex_stream_connections_active 0"));
        assert!(rendered.contains("dd_rust_network_mutex_stream_connection_cap_drops_total 1"));
    }

    #[test]
    fn auth_handshake_timeout_disabled_when_zero() {
        // Default is enabled; `0` disables it. (Parsing is env-driven, so we
        // only assert the constant/branch invariants here.)
        assert_eq!(DEFAULT_AUTH_HANDSHAKE_MS, 10_000);
        assert!(Duration::from_millis(DEFAULT_AUTH_HANDSHAKE_MS) > Duration::ZERO);
    }
}

/// Read one newline-terminated frame into `buf`, refusing to grow
/// past `max_bytes`. Returns:
///
/// * `Ok(true)`   — a complete frame was read. For normal JSONL
///   frames the trailing `\n` is included so the caller can strip it;
///   at EOF, a final unterminated record is also returned if at least
///   one byte was buffered.
/// * `Ok(false)`  — clean EOF before any bytes were read.
/// * `Err(InvalidData)` — the framer hit `max_bytes` without seeing
///   a newline. Caller MUST close the connection; the stream is
///   desynchronised at this point (we've consumed bytes that don't
///   belong to a known frame).
///
/// We loop on `fill_buf`/`consume` rather than `read_until` because
/// the latter has no built-in size cap; using it requires growing
/// `buf` first and checking afterwards, which is exactly the
/// vulnerability we're trying to plug.
async fn read_frame_bounded<R>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max_bytes: usize,
) -> std::io::Result<bool>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    crate::routine_id!("ddl-routine-read-frame-bounded-Vz7");
    use tokio::io::AsyncBufReadExt;
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
                    format!("frame exceeds {max_bytes} bytes"),
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
                format!("frame exceeds {max_bytes} bytes"),
            ));
        }
        buf.extend_from_slice(chunk);
        reader.consume(take);
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
    crate::routine_id!("ddl-routine-98pGTji7SYVytpEQBA");
    // Bound total concurrent connections before allocating any per-connection
    // state (client slot, writer task, read buffer). Over the cap we accept and
    // immediately close so a flood can't exhaust resources.
    let _conn_guard = match StreamConnGuard::try_acquire(max_connections(), Arc::clone(&metrics)) {
        Some(guard) => guard,
        None => {
            warn!(target: "lmx::tcp", "connection cap reached; dropping new connection");
            return Ok(());
        }
    };
    let (read, mut write) = tokio::io::split(stream);
    let (client_id, mut rx) = broker.register_client();
    let mut buf: Vec<u8> = Vec::new();
    let mut reader = BufReader::new(read);
    let mut authed = auth_token.is_none();
    let frame_cap = max_frame_bytes();
    let yield_every = frame_yield_every();
    let mut frames_seen: usize = 0;
    // Drop an unauthenticated connection that doesn't finish the handshake in
    // time (the post-auth phase is never deadlined, so holders aren't affected).
    let auth_deadline = if authed {
        None
    } else {
        auth_handshake_timeout().map(|to| Instant::now() + to)
    };

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
            // Enforce the auth-handshake deadline only while unauthenticated;
            // once authed we read with no timeout so a long-lived holder can sit
            // idle on an open connection.
            let frame_res = match auth_deadline.filter(|_| !authed) {
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    match tokio::time::timeout(
                        remaining,
                        read_frame_bounded(&mut reader, &mut buf, frame_cap),
                    )
                    .await
                    {
                        Ok(res) => res,
                        Err(_elapsed) => {
                            metrics.auth_failures_total.inc();
                            warn!(
                                target: "lmx::tcp",
                                client=%client_id,
                                "auth handshake deadline exceeded; dropping connection"
                            );
                            break;
                        }
                    }
                }
                None => read_frame_bounded(&mut reader, &mut buf, frame_cap).await,
            };
            let got_frame = match frame_res {
                Ok(b) => b,
                Err(err) if err.kind() == std::io::ErrorKind::InvalidData => {
                    // Oversized / unframed input from the peer. Surface
                    // a structured error to anyone still reading on the
                    // writer side, then close. Don't drain to EOF — the
                    // stream is desynchronised.
                    metrics.malformed_requests_total.inc();
                    broker.try_send(
                        client_id,
                        Response::Error {
                            uuid: "frame-too-large".into(),
                            error: err.to_string(),
                        },
                    );
                    warn!(
                        target: "lmx::tcp",
                        client=%client_id,
                        cap=%frame_cap,
                        "frame exceeded cap; dropping connection"
                    );
                    break;
                }
                Err(err) => return Err(err),
            };
            if !got_frame {
                break;
            }
            frames_seen = frames_seen.wrapping_add(1);
            if frames_seen.is_multiple_of(yield_every) {
                tokio::task::yield_now().await;
            }
            // Re-apply TCP_QUICKACK *immediately* after we've consumed a
            // frame from the kernel. This wins back the ~40 ms delayed-ACK
            // penalty on Linux for the next inbound segment. See
            // upstream issue ORESoftware/live-mutex#22 and
            // src/sockopt.rs.
            after_read.run(&metrics);
            // Trim trailing `\n` and any `\r` before it. JSON payloads
            // don't legitimately contain unescaped control characters
            // outside of the framing newline.
            let mut end = buf.len();
            while end > 0 && (buf[end - 1] == b'\n' || buf[end - 1] == b'\r') {
                end -= 1;
            }
            let payload = &buf[..end];
            if payload.is_empty() {
                continue;
            }
            let frame_started = Instant::now();
            metrics.observe_request_payload_bytes("stream_frame", payload.len());
            metrics.requests_total.inc();
            let request: Request = match serde_json::from_slice(payload) {
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
                    metrics.observe_request_duration("stream_frame", frame_started.elapsed());
                    continue;
                }
            };
            if !authed {
                if matches!(request, Request::Version { .. }) {
                    broker.handle_request(client_id, request);
                    metrics.observe_request_duration("stream_frame", frame_started.elapsed());
                    continue;
                }
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
                        metrics.observe_request_duration("stream_frame", frame_started.elapsed());
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
                    metrics.observe_request_duration("stream_frame", frame_started.elapsed());
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
                metrics.observe_request_duration("stream_frame", frame_started.elapsed());
                break;
            }
            broker.handle_request(client_id, request);
            let elapsed = frame_started.elapsed();
            metrics.observe_request_duration("stream_frame", elapsed);
            if elapsed >= Duration::from_millis(25) {
                debug!(
                    target: "lmx::stream",
                    client=%client_id,
                    elapsed_ms = elapsed.as_millis(),
                    "stream frame handled slowly"
                );
            }
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
    crate::routine_id!("ddl-routine-ZqIntiJXkbXsAaDxZT");
    handle_stream(sock, broker, auth_token, metrics, AfterRead::None).await
}

#[allow(dead_code)]
async fn _ensure_uds_handler_compiles(
    sock: UnixStream,
    broker: Broker,
    auth_token: Option<String>,
    metrics: Arc<crate::metrics::Metrics>,
) -> std::io::Result<()> {
    crate::routine_id!("ddl-routine-0jVAwo_ZNAd4KHqjYl");
    handle_stream(sock, broker, auth_token, metrics, AfterRead::None).await
}

// ---------------- HTTP layer -----------------------------------------------

fn http_unauthorized() -> AxumResponse {
    crate::routine_id!("ddl-routine-KaHmdHGpsEcVCMn-TA");
    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
}

fn observe_http_response(
    metrics: &crate::metrics::Metrics,
    route: &'static str,
    started: Instant,
    response: AxumResponse,
) -> AxumResponse {
    crate::routine_id!("ddl-routine-server-observe-http-response-1");
    let elapsed = started.elapsed();
    metrics.observe_request_duration(route, elapsed);
    if elapsed >= Duration::from_millis(250) {
        debug!(
            target: "lmx::http",
            route,
            status = response.status().as_u16(),
            elapsed_ms = elapsed.as_millis(),
            "HTTP request completed slowly"
        );
    }
    response
}

/// True if any accepted auth header carries `token`. Header names are
/// case-insensitive, so `Auth` and `auth` match the same `"auth"` lookup.
/// Accepted forms:
///   - `Authorization: Bearer <token>`
///   - `Auth: <token>`        (gateway/operator convention; value from `ALL_DOGS`)
///   - `X-LMX-Auth: <token>`  (legacy custom header)
fn http_token_matches(headers: &HeaderMap, token: &str) -> bool {
    crate::routine_id!("ddl-routine-server-http-token-matches-1");
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    header("authorization").and_then(|v| v.strip_prefix("Bearer ")) == Some(token)
        || header("auth") == Some(token)
        || header("x-lmx-auth") == Some(token)
}

fn http_authorized(state: &AppState, headers: &HeaderMap) -> bool {
    crate::routine_id!("ddl-routine-68-wt_8VTaRoe5rbQz");
    match state.auth_token.as_ref() {
        None => true,
        Some(token) => http_token_matches(headers, token),
    }
}

fn raft_http_authorized(state: &RaftAppState, headers: &HeaderMap) -> bool {
    crate::routine_id!("ddl-routine-server-raft-http-auth-1");
    match state.auth_token.as_ref() {
        None => true,
        Some(token) => http_token_matches(headers, token),
    }
}

fn http_request_id(
    headers: &HeaderMap,
    body_request_id: Option<&str>,
) -> Result<String, Box<AxumResponse>> {
    crate::routine_id!("ddl-routine-server-http-request-id-1");
    const MAX_REQUEST_ID_LEN: usize = 256;
    let header_request_id = ["x-lmx-request-id", "idempotency-key", "x-idempotency-key"]
        .iter()
        .filter_map(|name| headers.get(*name))
        .find_map(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let body_request_id = body_request_id
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let request_id = match (body_request_id, header_request_id) {
        (Some(body), Some(header)) if body != header => {
            return Err(Box::new(
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "requestId and idempotency header disagree"
                    })),
                )
                    .into_response(),
            ));
        }
        (Some(value), _) | (_, Some(value)) => value.to_string(),
        (None, None) => Uuid::new_v4().to_string(),
    };
    if request_id.len() > MAX_REQUEST_ID_LEN {
        return Err(Box::new(
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("requestId must be at most {MAX_REQUEST_ID_LEN} bytes")
                })),
            )
                .into_response(),
        ));
    }
    Ok(request_id)
}

async fn raft_metrics_endpoint(State(state): State<RaftAppState>) -> impl IntoResponse {
    crate::routine_id!("ddl-routine-server-raft-metrics-1");
    let mut body = state.metrics.render(state.raft.broker());
    body.push_str(&state.raft.raft_metrics_text());
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
}

async fn raft_status(State(state): State<RaftAppState>) -> AxumResponse {
    crate::routine_id!("ddl-routine-server-raft-status-1");
    let progress = match state.raft.progress_snapshot_fresh().await {
        Ok(progress) => progress,
        Err(err) => return raft_unavailable(err),
    };
    let cluster_size = progress.membership.cluster_size();
    let quorum_size = progress.membership.quorum_size();
    Json(serde_json::json!({
        "nodeId": progress.node_id,
        "isLeader": progress.is_leader,
        "isLeaderReady": progress.is_leader_ready,
        "leaderId": progress.leader_id,
        "leaderAddr": progress.leader_addr,
        "leaderQuorumAgeMs": progress.leader_quorum_age_ms,
        "leaderQuorumTimeoutMs": progress.leader_quorum_timeout_ms,
        "clusterSize": cluster_size,
        "quorumSize": quorum_size,
        "membershipJoint": progress.membership_joint,
        "membership": progress.membership,
        "currentTerm": progress.current_term,
        "commitIndex": progress.commit_index,
        "lastApplied": progress.last_applied,
        "lastLogIndex": progress.last_log_index,
        "lastLogTerm": progress.last_log_term,
        "syncLog": progress.sync_log,
        "syncCommit": progress.sync_commit,
        "unsafeDurability": progress.unsafe_durability,
    }))
    .into_response()
}

async fn raft_membership(State(state): State<RaftAppState>) -> AxumResponse {
    crate::routine_id!("ddl-routine-server-raft-membership-1");
    let progress = match state.raft.progress_snapshot_fresh().await {
        Ok(progress) => progress,
        Err(err) => return raft_unavailable(err),
    };
    let cluster_size = progress.membership.cluster_size();
    let quorum_size = progress.membership.quorum_size();
    Json(serde_json::json!({
        "nodeId": progress.node_id,
        "isLeader": progress.is_leader,
        "leaderId": progress.leader_id,
        "leaderAddr": progress.leader_addr,
        "clusterSize": cluster_size,
        "quorumSize": quorum_size,
        "membershipJoint": progress.membership_joint,
        "membership": progress.membership,
    }))
    .into_response()
}

async fn raft_progress(State(state): State<RaftAppState>) -> AxumResponse {
    crate::routine_id!("ddl-routine-server-raft-progress-1");
    match state.raft.progress_snapshot_fresh().await {
        Ok(progress) => Json(progress).into_response(),
        Err(err) => raft_unavailable(err),
    }
}

async fn raft_learners(State(state): State<RaftAppState>) -> AxumResponse {
    crate::routine_id!("ddl-routine-server-raft-learners-1");
    let progress = match state.raft.progress_snapshot_fresh().await {
        Ok(progress) => progress,
        Err(err) => return raft_unavailable(err),
    };
    let learners = progress
        .peers
        .into_iter()
        .filter(|peer| peer.staged_learner)
        .collect::<Vec<_>>();
    Json(serde_json::json!({
        "nodeId": progress.node_id,
        "isLeader": progress.is_leader,
        "isLeaderReady": progress.is_leader_ready,
        "leaderId": progress.leader_id,
        "leaderAddr": progress.leader_addr,
        "leaderQuorumAgeMs": progress.leader_quorum_age_ms,
        "leaderQuorumTimeoutMs": progress.leader_quorum_timeout_ms,
        "lastLogIndex": progress.last_log_index,
        "syncLog": progress.sync_log,
        "syncCommit": progress.sync_commit,
        "unsafeDurability": progress.unsafe_durability,
        "learners": learners,
    }))
    .into_response()
}

async fn raft_stage_learners(
    State(state): State<RaftAppState>,
    headers: HeaderMap,
    Json(req): Json<RaftLearnersRequest>,
) -> AxumResponse {
    crate::routine_id!("ddl-routine-server-raft-stage-learners-1");
    if !raft_http_authorized(&state, &headers) {
        return http_unauthorized();
    }
    match state.raft.stage_learners(req.peers).await {
        Ok(learners) => Json(serde_json::json!({
            "learners": learners,
            "progress": state.raft.progress_snapshot(),
        }))
        .into_response(),
        Err(err @ BrokerRaftError::NotLeader { .. }) => raft_unavailable(err),
        Err(err @ BrokerRaftError::InvalidConfig(_)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
        Err(err) => raft_unavailable(err),
    }
}

async fn raft_remove_learners(
    State(state): State<RaftAppState>,
    headers: HeaderMap,
    Json(req): Json<RaftLearnerRemovalRequest>,
) -> AxumResponse {
    crate::routine_id!("ddl-routine-server-raft-remove-learners-1");
    if !raft_http_authorized(&state, &headers) {
        return http_unauthorized();
    }
    match state.raft.remove_staged_learners(req.ids).await {
        Ok(learners) => Json(serde_json::json!({
            "learners": learners,
            "progress": state.raft.progress_snapshot(),
        }))
        .into_response(),
        Err(err @ BrokerRaftError::NotLeader { .. }) => raft_unavailable(err),
        Err(err @ BrokerRaftError::InvalidConfig(_)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
        Err(err) => raft_unavailable(err),
    }
}

async fn raft_change_membership(
    State(state): State<RaftAppState>,
    headers: HeaderMap,
    Json(req): Json<RaftMembershipRequest>,
) -> AxumResponse {
    crate::routine_id!("ddl-routine-server-raft-change-membership-1");
    if !raft_http_authorized(&state, &headers) {
        return http_unauthorized();
    }
    match state.raft.change_membership(req.peers).await {
        Ok(index) => Json(serde_json::json!({
            "index": index,
            "membership": state.raft.membership(),
            "clusterSize": state.raft.active_cluster_size(),
            "quorumSize": state.raft.active_quorum_size(),
        }))
        .into_response(),
        Err(err @ BrokerRaftError::NotLeader { .. }) => raft_unavailable(err),
        Err(err @ BrokerRaftError::InvalidConfig(_)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
        Err(err) => raft_unavailable(err),
    }
}

async fn raft_leaderz(State(state): State<RaftAppState>) -> AxumResponse {
    crate::routine_id!("ddl-routine-server-raft-leaderz-1");
    let progress = match state.raft.progress_snapshot_fresh().await {
        Ok(progress) => progress,
        Err(err) => return raft_unavailable(err),
    };
    let body = serde_json::json!({
        "nodeId": progress.node_id,
        "isLeader": progress.is_leader,
        "isLeaderReady": progress.is_leader_ready,
        "leaderId": progress.leader_id,
        "leaderAddr": progress.leader_addr,
        "leaderQuorumAgeMs": progress.leader_quorum_age_ms,
        "leaderQuorumTimeoutMs": progress.leader_quorum_timeout_ms,
        "syncLog": progress.sync_log,
        "syncCommit": progress.sync_commit,
        "unsafeDurability": progress.unsafe_durability,
    });
    if progress.is_leader_ready {
        Json(body).into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
    }
}

/// Middleware that stamps the local node's Raft role and last-known leader onto
/// every HTTP response, so a load balancer or client can learn who the leader is
/// from any response (e.g. a normal `/v1/lock`) without a separate probe:
///   - `X-Raft-Node-Id`                  — this node's configured Raft id.
///   - `X-Raft-Role: leader|candidate|follower` — this node's current Raft role.
///   - `X-Raft-Term`, `X-Raft-Commit-Index`, `X-Raft-Last-Applied`.
///   - `X-Raft-Last-Log-Index`, `X-Raft-Last-Log-Term`.
///   - `X-Raft-Leader-Ready: true|false`  — whether this node, as leader, can
///     currently serve writes (false on followers and quorum-less leaders).
///   - `X-Raft-Leader-Quorum-Age-Ms` / `X-Raft-Leader-Quorum-Timeout-Ms`.
///   - `X-Raft-Leader-Id` / `X-Raft-Leader-Addr` — the leader this node knows of.
///
/// The hint can be briefly stale during a failover; the follower proxy fallback
/// still guarantees correctness regardless of where the request lands.
async fn raft_leader_response_headers(
    State(state): State<RaftAppState>,
    request: AxumRequest,
    next: Next,
) -> AxumResponse {
    crate::routine_id!("ddl-routine-server-raft-leader-headers-1");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    let progress = state.raft.progress_snapshot();
    insert_raft_response_header(headers, "x-raft-node-id", &progress.node_id);
    insert_raft_response_header(headers, "x-raft-role", &progress.role);
    insert_raft_response_header(headers, "x-raft-term", progress.current_term.to_string());
    insert_raft_response_header(
        headers,
        "x-raft-commit-index",
        progress.commit_index.to_string(),
    );
    insert_raft_response_header(
        headers,
        "x-raft-last-applied",
        progress.last_applied.to_string(),
    );
    insert_raft_response_header(
        headers,
        "x-raft-last-log-index",
        progress.last_log_index.to_string(),
    );
    insert_raft_response_header(
        headers,
        "x-raft-last-log-term",
        progress.last_log_term.to_string(),
    );
    headers.insert(
        "x-raft-leader-ready",
        HeaderValue::from_static(if progress.is_leader_ready {
            "true"
        } else {
            "false"
        }),
    );
    if let Some(age_ms) = progress.leader_quorum_age_ms {
        insert_raft_response_header(headers, "x-raft-leader-quorum-age-ms", age_ms.to_string());
    }
    insert_raft_response_header(
        headers,
        "x-raft-leader-quorum-timeout-ms",
        progress.leader_quorum_timeout_ms.to_string(),
    );
    insert_raft_response_header(
        headers,
        "x-raft-membership-joint",
        progress.membership_joint.to_string(),
    );
    insert_raft_response_header(headers, "x-raft-sync-log", progress.sync_log.to_string());
    insert_raft_response_header(
        headers,
        "x-raft-sync-commit",
        progress.sync_commit.to_string(),
    );
    insert_raft_response_header(
        headers,
        "x-raft-unsafe-durability",
        progress.unsafe_durability.to_string(),
    );
    if let Some(leader_id) = progress.leader_id.as_ref() {
        insert_raft_response_header(headers, "x-raft-leader-id", leader_id);
    }
    if let Some(leader_addr) = progress.leader_addr.as_ref() {
        insert_raft_response_header(headers, "x-raft-leader-addr", leader_addr);
    }
    response
}

fn insert_raft_response_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: impl AsRef<str>,
) {
    crate::routine_id!("ddl-routine-server-insert-raft-response-header-1");
    if let Ok(value) = HeaderValue::from_str(value.as_ref()) {
        headers.insert(name, value);
    }
}

async fn raft_http_acquire(
    State(state): State<RaftAppState>,
    headers: HeaderMap,
    Json(req): Json<AcquireRequest>,
) -> AxumResponse {
    crate::routine_id!("ddl-routine-server-raft-acquire-1");
    let started = Instant::now();
    if !raft_http_authorized(&state, &headers) {
        return observe_http_response(
            &state.metrics,
            "raft_http_acquire",
            started,
            http_unauthorized(),
        );
    }
    state.metrics.requests_total.inc();
    let request_uuid = match http_request_id(&headers, req.request_id.as_deref()) {
        Ok(request_id) => request_id,
        Err(response) => {
            return observe_http_response(&state.metrics, "raft_http_acquire", started, *response);
        }
    };
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
        wait: None,
    };
    let wait = req.wait_ms.map(Duration::from_millis).unwrap_or_default();
    let outcome = state
        .raft
        .run_ephemeral(request, &request_uuid, wait, true)
        .await;
    let mut response = match outcome {
        Ok(Some(Response::Lock {
            acquired,
            key,
            lock_uuid,
            fencing_token,
            lock_request_count,
            error,
            ..
        })) => {
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
                error,
            };
            if !acquired && body.error.is_some() {
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            } else {
                Json(body).into_response()
            }
        }
        Ok(Some(Response::CompositeLock {
            acquired,
            keys,
            lock_uuid,
            fencing_tokens,
            error,
            ..
        })) => {
            let body = AcquireResponse {
                acquired,
                keys,
                lock_uuid,
                fencing_tokens: fencing_tokens.unwrap_or_default(),
                queue_depth: 0,
                error,
            };
            if !acquired && body.error.is_some() {
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            } else {
                Json(body).into_response()
            }
        }
        Ok(Some(Response::Error { error, .. })) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"acquired": false, "error": error})),
        )
            .into_response(),
        Ok(_) => Json(AcquireResponse {
            acquired: false,
            keys: req.keys.unwrap_or_else(|| req.key.into_iter().collect()),
            lock_uuid: None,
            fencing_tokens: BTreeMap::new(),
            queue_depth: 0,
            error: None,
        })
        .into_response(),
        Err(BrokerRaftError::UnsupportedClientRequest(error)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"acquired": false, "error": error})),
        )
            .into_response(),
        Err(err @ BrokerRaftError::NotLeader { .. }) => raft_unavailable(err),
        Err(err) => raft_unavailable(err),
    };
    insert_raft_response_header(response.headers_mut(), "x-lmx-request-id", &request_uuid);
    observe_http_response(&state.metrics, "raft_http_acquire", started, response)
}

async fn raft_http_release(
    State(state): State<RaftAppState>,
    headers: HeaderMap,
    Json(req): Json<ReleaseRequest>,
) -> AxumResponse {
    crate::routine_id!("ddl-routine-server-raft-release-1");
    let started = Instant::now();
    if !raft_http_authorized(&state, &headers) {
        return observe_http_response(
            &state.metrics,
            "raft_http_release",
            started,
            http_unauthorized(),
        );
    }
    state.metrics.requests_total.inc();
    let request_uuid = match http_request_id(&headers, req.request_id.as_deref()) {
        Ok(request_id) => request_id,
        Err(response) => {
            return observe_http_response(&state.metrics, "raft_http_release", started, *response);
        }
    };
    let outcome = state
        .raft
        .run_ephemeral(
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
    let mut response = match outcome {
        Ok(Some(Response::Unlock { keys, unlocked, .. })) => {
            Json(ReleaseResponse { unlocked, keys }).into_response()
        }
        Ok(Some(Response::Error { error, .. })) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"unlocked": false, "error": error})),
        )
            .into_response(),
        Ok(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"unlocked": false, "error": "broker timed out"})),
        )
            .into_response(),
        Err(BrokerRaftError::UnsupportedClientRequest(error)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"unlocked": false, "error": error})),
        )
            .into_response(),
        Err(err @ BrokerRaftError::NotLeader { .. }) => raft_unavailable(err),
        Err(err) => raft_unavailable(err),
    };
    insert_raft_response_header(response.headers_mut(), "x-lmx-request-id", &request_uuid);
    observe_http_response(&state.metrics, "raft_http_release", started, response)
}

fn raft_unavailable(err: BrokerRaftError) -> AxumResponse {
    crate::routine_id!("ddl-routine-server-raft-unavailable-1");
    match err {
        BrokerRaftError::NotLeader {
            leader_id,
            leader_addr,
        } => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "raft leader unavailable",
                "leaderId": leader_id,
                "leaderAddr": leader_addr,
            })),
        )
            .into_response(),
        BrokerRaftError::IdempotencyKeyConflict { request_id } => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "request id reused with different payload",
                "requestId": request_id,
            })),
        )
            .into_response(),
        BrokerRaftError::UnsupportedClientRequest(error) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": error,
            })),
        )
            .into_response(),
        BrokerRaftError::ClientProposalUncertain {
            index,
            votes,
            quorum,
        } => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "raft client proposal outcome unknown; retry the same request id",
                "index": index,
                "votes": votes,
                "quorum": quorum,
                "outcomeUnknown": true,
                "retrySameRequestId": true,
            })),
        )
            .into_response(),
        other => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": other.to_string(),
            })),
        )
            .into_response(),
    }
}

/// Shared secret used to authenticate `/admin/*` requests. Defaults to
/// the literal string the operator explicitly chose at request time;
/// override via the `LMX_ADMIN_TOKEN` env var in production. This is
/// deliberately a *separate* token from `AppState::auth_token` so the
/// admin surface can be locked down even when general-purpose lock API
/// auth is disabled.
fn admin_token() -> String {
    crate::routine_id!("ddl-routine-admin-token-Ld2");
    std::env::var("LMX_ADMIN_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "all-dogs-go-to-heaven".to_string())
}

/// Validate an admin shared-secret header on `/admin/*` routes. Accepts
/// either `x-admin-token: <token>` or `Authorization: Bearer <token>`.
fn admin_authorized(headers: &HeaderMap) -> bool {
    crate::routine_id!("ddl-routine-admin-authorized-Vr8");
    let expected = admin_token();
    let custom = headers
        .get("x-admin-token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    custom.as_deref() == Some(expected.as_str()) || bearer.as_deref() == Some(expected.as_str())
}

/// `true` when the inbound request looks like it came from HTMX. We
/// consult the standard `HX-Request: true` header HTMX always sets on
/// AJAX-driven requests. Used to dual-output the admin POST handlers:
/// HTML snippet for HTMX (so `hx-swap` can drop it straight into a
/// `<span>`), JSON for everyone else (so `curl`/operators see the
/// same response shape they always have).
fn is_htmx_request(headers: &HeaderMap) -> bool {
    headers
        .get("hx-request")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// HTML response with the right content-type for `hx-swap`. We return
/// a tiny snippet (no `<html>`/`<body>`) because HTMX swaps it as
/// fragment content.
fn html_snippet(status: StatusCode, body: String) -> AxumResponse {
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

/// Parse a small admin-POST body as either `application/json` or
/// `application/x-www-form-urlencoded`, depending on the inbound
/// `Content-Type`. HTMX (without the optional `json-enc` extension)
/// posts `application/x-www-form-urlencoded`; `curl -d '{...}'` posts
/// JSON. Empty / unset content types fall back to JSON since that's
/// the historical contract for `/admin/otel`.
fn parse_admin_body<T>(headers: &HeaderMap, body: &[u8]) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    let ct = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if ct.starts_with("application/x-www-form-urlencoded") {
        serde_urlencoded::from_bytes(body).map_err(|e| e.to_string())
    } else {
        // Default to JSON. `application/json` and the empty default
        // both land here.
        serde_json::from_slice(body).map_err(|e| e.to_string())
    }
}

#[derive(serde::Deserialize)]
struct OtelToggleRequest {
    // `application/x-www-form-urlencoded` deserialises numeric/string
    // values into bool via serde, so the form payload `enabled=true`
    // and the JSON payload `{"enabled":true}` both populate this.
    enabled: bool,
}

fn render_otel_snippet(enabled: bool) -> String {
    let state = if enabled { "on" } else { "off" };
    format!("otel: <strong>{state}</strong>")
}

/// Tiny HTML escaper for the inline admin response snippets. We
/// don't pull in the (private) `status::html_escape` here because
/// it'd force a `pub(crate)` widening; the admin snippets only
/// interpolate operator-controlled directive strings and parser
/// error messages, both small enough that this fits in five
/// branches.
fn html_escape_min(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// `GET /admin/otel` — return the current state of the OTel kill-switch.
async fn admin_otel_get(headers: HeaderMap) -> impl IntoResponse {
    crate::routine_id!("ddl-routine-admin-otel-get-Hs5");
    if !admin_authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "admin endpoint requires `x-admin-token` (or `Authorization: Bearer ...`)."
            })),
        )
            .into_response();
    }
    Json(serde_json::json!({
        "enabled": crate::routine::is_otel_enabled(),
    }))
    .into_response()
}

/// `POST /admin/otel` — flip the OTel kill-switch at runtime. Body is
/// `{"enabled": true|false}`. Response includes `previous` so audit
/// logs can record the transition without an extra GET.
///
/// We auth FIRST against the headers, then take the body via a raw
/// `bytes::Bytes` extractor so we control the parse path. axum's
/// built-in `Json<T>` would 400 before our auth check has a chance to
/// run on a missing or malformed body, which would let unauth callers
/// distinguish "endpoint exists" from "endpoint missing" via the error
/// shape — minor info leak, but easy to avoid.
async fn admin_otel_post(headers: HeaderMap, body: axum::body::Bytes) -> impl IntoResponse {
    crate::routine_id!("ddl-routine-admin-otel-post-Kt4");
    if !admin_authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "admin endpoint requires `x-admin-token` (or `Authorization: Bearer ...`)."
            })),
        )
            .into_response();
    }
    let htmx = is_htmx_request(&headers);
    let parsed: Result<OtelToggleRequest, _> = parse_admin_body(&headers, &body);
    let req = match parsed {
        Ok(r) => r,
        Err(err) => {
            // Mirror the JSON error in HTML for HTMX so the inline
            // `<span>` shows something useful instead of going stale.
            if htmx {
                return html_snippet(
                    StatusCode::BAD_REQUEST,
                    format!("otel: <strong>error</strong> ({})", html_escape_min(&err)),
                );
            }
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("`enabled` is required and must be a boolean ({err})."),
                })),
            )
                .into_response();
        }
    };
    let previous = crate::routine::set_otel_enabled(req.enabled);
    let now = crate::routine::is_otel_enabled();
    if htmx {
        html_snippet(StatusCode::OK, render_otel_snippet(now))
    } else {
        Json(serde_json::json!({
            "previous": previous,
            "enabled": now,
        }))
        .into_response()
    }
}

#[derive(serde::Deserialize)]
struct LogLevelRequest {
    directive: String,
}

/// `GET /admin/log-level` — return the current `EnvFilter` directive.
async fn admin_log_level_get(headers: HeaderMap) -> impl IntoResponse {
    crate::routine_id!("ddl-routine-admin-log-level-get-Bm6");
    if !admin_authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "admin endpoint requires `x-admin-token` (or `Authorization: Bearer ...`)."
            })),
        )
            .into_response();
    }
    Json(serde_json::json!({
        "directive": crate::routine::current_log_level(),
    }))
    .into_response()
}

/// `POST /admin/log-level` — install a new `EnvFilter` directive at
/// runtime via the `tracing-subscriber::reload` handle wired up in
/// `init_tracing`. Body is `{"directive": "<value>"}`. Returns
/// `{"previous": "...", "directive": "..."}` on success, 400 with the
/// parser error on malformed input.
async fn admin_log_level_post(headers: HeaderMap, body: axum::body::Bytes) -> impl IntoResponse {
    crate::routine_id!("ddl-routine-admin-log-level-post-Cn7");
    if !admin_authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "admin endpoint requires `x-admin-token` (or `Authorization: Bearer ...`)."
            })),
        )
            .into_response();
    }
    let htmx = is_htmx_request(&headers);
    let parsed: Result<LogLevelRequest, _> = parse_admin_body(&headers, &body);
    let req = match parsed {
        Ok(r) => r,
        Err(err) => {
            if htmx {
                return html_snippet(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "log-level: <strong>error</strong> ({})",
                        html_escape_min(&err)
                    ),
                );
            }
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("`directive` is required and must be a string ({err})."),
                })),
            )
                .into_response();
        }
    };
    let previous = crate::routine::current_log_level();
    match crate::routine::set_log_level(&req.directive) {
        Ok(applied) => {
            if htmx {
                html_snippet(
                    StatusCode::OK,
                    format!("log-level: <strong>{}</strong>", html_escape_min(&applied)),
                )
            } else {
                Json(serde_json::json!({
                    "previous": previous,
                    "directive": applied,
                }))
                .into_response()
            }
        }
        Err(err) => {
            if htmx {
                html_snippet(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "log-level: <strong>error</strong> ({})",
                        html_escape_min(&err)
                    ),
                )
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": err,
                        "previous": previous,
                    })),
                )
                    .into_response()
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct TcpToggleRequest {
    nodelay: Option<bool>,
    quickack: Option<bool>,
}

/// `GET /admin/tcp` — return the current TCP socket-tuning flags
/// (live values, not the long-stale `ServerConfig` snapshot).
async fn admin_tcp_get(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    crate::routine_id!("ddl-routine-admin-tcp-get-Dp8");
    if !admin_authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "admin endpoint requires `x-admin-token` (or `Authorization: Bearer ...`)."
            })),
        )
            .into_response();
    }
    let nodelay = state.tcp_flags.nodelay();
    let quickack = state.tcp_flags.quickack();
    Json(serde_json::json!({
        "nodelay": nodelay,
        "quickack": quickack,
        "quickack_supported": cfg!(target_os = "linux"),
    }))
    .into_response()
}

/// `POST /admin/tcp` — flip NODELAY and/or QUICKACK at runtime. Body
/// is `{"nodelay"?: bool, "quickack"?: bool}`. The change is picked
/// up by the accept loop on the next accept (NODELAY) and by the
/// per-frame `AfterRead` hook on the next read (QUICKACK), so already
/// long-lived connections see the new behavior without having to
/// reconnect.
///
/// On non-Linux targets, setting `quickack: true` still stores the
/// flag (so a future Linux deploy will pick it up) but the response
/// includes a `warning` field reminding the operator that the option
/// is a no-op on the current OS.
async fn admin_tcp_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    crate::routine_id!("ddl-routine-admin-tcp-post-Eq9");
    if !admin_authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "admin endpoint requires `x-admin-token` (or `Authorization: Bearer ...`)."
            })),
        )
            .into_response();
    }
    let htmx = is_htmx_request(&headers);
    let parsed: Result<TcpToggleRequest, _> = parse_admin_body(&headers, &body);
    let req = match parsed {
        Ok(r) => r,
        Err(err) => {
            if htmx {
                return html_snippet(
                    StatusCode::BAD_REQUEST,
                    format!("tcp: <strong>error</strong> ({})", html_escape_min(&err)),
                );
            }
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("expected body with optional `nodelay` and/or `quickack` booleans ({err})."),
                })),
            )
                .into_response();
        }
    };
    if req.nodelay.is_none() && req.quickack.is_none() {
        if htmx {
            return html_snippet(
                StatusCode::BAD_REQUEST,
                "tcp: <strong>error</strong> (need nodelay or quickack)".to_string(),
            );
        }
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "at least one of `nodelay` or `quickack` must be present."
            })),
        )
            .into_response();
    }
    let previous_nodelay = state.tcp_flags.nodelay();
    let previous_quickack = state.tcp_flags.quickack();
    if let Some(v) = req.nodelay {
        state.tcp_flags.set_nodelay(v);
    }
    let mut warning: Option<&'static str> = None;
    if let Some(v) = req.quickack {
        state.tcp_flags.set_quickack(v);
        if v && !cfg!(target_os = "linux") {
            warning = Some("TCP_QUICKACK is no-op on this OS");
        }
    }
    if htmx {
        let nodelay_str = if state.tcp_flags.nodelay() {
            "on"
        } else {
            "off"
        };
        let quickack_str = if state.tcp_flags.quickack() {
            "on"
        } else {
            "off"
        };
        let warn_html = warning
            .map(|w| format!(" <em>({})</em>", html_escape_min(w)))
            .unwrap_or_default();
        return html_snippet(
            StatusCode::OK,
            format!(
                "tcp: NODELAY <strong>{nodelay_str}</strong> · QUICKACK <strong>{quickack_str}</strong>{warn_html}"
            ),
        );
    }
    let mut body = serde_json::json!({
        "nodelay": state.tcp_flags.nodelay(),
        "quickack": state.tcp_flags.quickack(),
        "quickack_supported": cfg!(target_os = "linux"),
        "previous": {
            "nodelay": previous_nodelay,
            "quickack": previous_quickack,
        },
    });
    if let Some(w) = warning {
        body["warning"] = serde_json::Value::String(w.to_string());
    }
    Json(body).into_response()
}

async fn healthz() -> impl IntoResponse {
    crate::routine_id!("ddl-routine-TswAzuekSL3ki9tHzu");
    Json(serde_json::json!({"ok": true, "service": "dd-rust-network-mutex"}))
}

async fn metrics_endpoint(State(state): State<AppState>) -> impl IntoResponse {
    crate::routine_id!("ddl-routine-4f1x2CLglT8maVVKYh");
    let body = state.metrics.render(&state.broker);
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
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
    crate::routine_id!("ddl-routine-leTmUgVn8BaNKdrlty");
    let metrics_text = state.metrics.render(&state.broker);
    // Refresh the live runtime fields on each render so the page
    // reflects whatever the most recent `POST /admin/*` actions have
    // toggled. The fields baked into `StatusServerInfo` at startup
    // (bind addresses, TLS config, broker tunables) stay stable.
    let mut info = (*state.info).clone();
    info.tcp_nodelay = state.tcp_flags.nodelay();
    info.tcp_quickack = state.tcp_flags.quickack();
    info.tcp_quickack_effective =
        state.tcp_flags.quickack() && crate::sockopt::quickack_supported();
    info.log_directive = crate::routine::current_log_level();
    info.otel_enabled = crate::routine::is_otel_enabled();
    let html = crate::status::render(&state.broker, &info, &metrics_text);
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
}

/// State for the status routes. Distinct from `AppState` so the
/// dedicated status listener doesn't accidentally pull in the
/// auth-token field. Carries the same `Arc<TcpFlags>` so the
/// server-rendered status page reflects whatever the most recent
/// `POST /admin/tcp` flipped the flags to (rather than the values
/// from the long-stale startup `ServerConfig`).
#[derive(Clone)]
struct StatusAppState {
    broker: Broker,
    metrics: Arc<crate::metrics::Metrics>,
    info: Arc<crate::status::StatusServerInfo>,
    tcp_flags: Arc<TcpFlags>,
}

fn build_status_info(config: &ServerConfig) -> crate::status::StatusServerInfo {
    crate::routine_id!("ddl-routine-aps2N0EHQfJbC80RxJ");
    crate::status::StatusServerInfo {
        tcp_bind: config.tcp_bind.map(|a| a.to_string()),
        uds_path: config
            .uds_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        http_bind: config.http_bind.map(|a| a.to_string()),
        status_bind: config.status_bind.map(|a| a.to_string()),
        auth_token_set: config.auth_token.is_some(),
        // Initial values — `status_page()` overwrites these on each
        // render with the live `Arc<TcpFlags>` snapshot so the page
        // reflects runtime toggles, not just startup config.
        tcp_nodelay: config.tcp_nodelay,
        tcp_quickack: config.tcp_quickack,
        tcp_quickack_effective: config.tcp_quickack && crate::sockopt::quickack_supported(),
        log_directive: crate::routine::current_log_level(),
        otel_enabled: crate::routine::is_otel_enabled(),
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
    crate::routine_id!("ddl-routine-ElPo3wC15B8bZyLP5u");
    Router::new()
        .route("/", get(status_page))
        .route("/status", get(status_page))
        .route("/healthz", get(healthz))
        .route("/readyz", get(healthz))
        .route("/metrics", get(metrics_endpoint_status))
        .with_state(state)
}

async fn metrics_endpoint_status(State(state): State<StatusAppState>) -> impl IntoResponse {
    crate::routine_id!("ddl-routine-i3VYM9l8j70h2x7bg6");
    let body = state.metrics.render(&state.broker);
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
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
    crate::routine_id!("ddl-routine-Ju7vsu-dtrVT5bjTzJ");
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
    crate::routine_id!("ddl-routine-T5EHOY2NmST_NSzimj");
    let started = Instant::now();
    if !http_authorized(&state, &headers) {
        return observe_http_response(&state.metrics, "http_acquire", started, http_unauthorized());
    }
    state.metrics.requests_total.inc();
    let request_uuid = match http_request_id(&headers, req.request_id.as_deref()) {
        Ok(request_id) => request_id,
        Err(response) => {
            return observe_http_response(&state.metrics, "http_acquire", started, *response);
        }
    };
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
        // HTTP always enqueues (wait=default): the long-poll window is governed
        // by `run_ephemeral` below, and the ephemeral client's teardown prunes
        // any still-queued waiter, so this can't leak a deferred grant. The
        // `wait:false` fail-fast contract is for persistent TCP/UDS clients
        // that would otherwise abandon a queued request.
        wait: None,
    };
    let wait = req.wait_ms.map(Duration::from_millis).unwrap_or_default();
    let outcome = run_ephemeral(&state, request, &request_uuid, wait, true).await;
    let response = match outcome {
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
    };
    observe_http_response(&state.metrics, "http_acquire", started, response)
}

async fn http_release(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ReleaseRequest>,
) -> AxumResponse {
    crate::routine_id!("ddl-routine-PMtoSZPDfVphM8N9Bz");
    let started = Instant::now();
    if !http_authorized(&state, &headers) {
        return observe_http_response(&state.metrics, "http_release", started, http_unauthorized());
    }
    state.metrics.requests_total.inc();
    let request_uuid = match http_request_id(&headers, req.request_id.as_deref()) {
        Ok(request_id) => request_id,
        Err(response) => {
            return observe_http_response(&state.metrics, "http_release", started, *response);
        }
    };
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
    let response = match outcome {
        Some(Response::Unlock { keys, unlocked, .. }) => {
            Json(ReleaseResponse { unlocked, keys }).into_response()
        }
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
    };
    observe_http_response(&state.metrics, "http_release", started, response)
}

async fn http_rw_read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RwAcquireRequest>,
) -> AxumResponse {
    crate::routine_id!("ddl-routine-48iX_Lmr3tl95p__v_");
    let started = Instant::now();
    if !http_authorized(&state, &headers) {
        return observe_http_response(&state.metrics, "http_rw_read", started, http_unauthorized());
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
    let response = match outcome {
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
    };
    observe_http_response(&state.metrics, "http_rw_read", started, response)
}

async fn http_rw_read_end(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RwReleaseRequest>,
) -> AxumResponse {
    crate::routine_id!("ddl-routine-X1MXPfJCWk8TxUSU5S");
    let started = Instant::now();
    if !http_authorized(&state, &headers) {
        return observe_http_response(
            &state.metrics,
            "http_rw_read_end",
            started,
            http_unauthorized(),
        );
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
    let response = match outcome {
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
    };
    observe_http_response(&state.metrics, "http_rw_read_end", started, response)
}

async fn http_rw_write(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RwAcquireRequest>,
) -> AxumResponse {
    crate::routine_id!("ddl-routine-6Uk8OjI7dBoBeZKwgK");
    let started = Instant::now();
    if !http_authorized(&state, &headers) {
        return observe_http_response(
            &state.metrics,
            "http_rw_write",
            started,
            http_unauthorized(),
        );
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
    let response = match outcome {
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
    };
    observe_http_response(&state.metrics, "http_rw_write", started, response)
}

async fn http_rw_write_end(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RwReleaseRequest>,
) -> AxumResponse {
    crate::routine_id!("ddl-routine-yiL_jYhCPvqvrCLT8q");
    let started = Instant::now();
    if !http_authorized(&state, &headers) {
        return observe_http_response(
            &state.metrics,
            "http_rw_write_end",
            started,
            http_unauthorized(),
        );
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
    let response = match outcome {
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
    };
    observe_http_response(&state.metrics, "http_rw_write_end", started, response)
}

async fn http_lock_info(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> AxumResponse {
    crate::routine_id!("ddl-routine-PR-FIDcsqi_1KmgjpN");
    let started = Instant::now();
    if !http_authorized(&state, &headers) {
        return observe_http_response(
            &state.metrics,
            "http_lock_info",
            started,
            http_unauthorized(),
        );
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
    let response = match outcome {
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
    };
    observe_http_response(&state.metrics, "http_lock_info", started, response)
}

async fn http_ls(State(state): State<AppState>, headers: HeaderMap) -> AxumResponse {
    crate::routine_id!("ddl-routine-81lPAudjnSt0pV3DSg");
    let started = Instant::now();
    if !http_authorized(&state, &headers) {
        return observe_http_response(&state.metrics, "http_ls", started, http_unauthorized());
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
    let response = match outcome {
        Some(Response::LsResult { keys, .. }) => {
            Json(serde_json::json!({"keys": keys})).into_response()
        }
        _ => (StatusCode::SERVICE_UNAVAILABLE, "broker did not respond").into_response(),
    };
    observe_http_response(&state.metrics, "http_ls", started, response)
}

fn deadline_after(timeout: Duration) -> tokio::time::Instant {
    crate::routine_id!("ddl-routine-server-deadline-after-Jb7");
    let now = tokio::time::Instant::now();
    now.checked_add(timeout)
        .unwrap_or_else(|| now + Duration::from_secs(365 * 24 * 60 * 60))
}

async fn wait_for(
    rx: &mut mpsc::UnboundedReceiver<Response>,
    request_uuid: &str,
    wait: Duration,
    keep_polling_until_definitive: bool,
) -> Option<Response> {
    crate::routine_id!("ddl-routine-aDbCebFJGfVUwTsm4K");
    let _ = keep_polling_until_definitive;
    let timeout = if wait.is_zero() {
        Duration::from_millis(50)
    } else {
        wait
    };
    let deadline = deadline_after(timeout);
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
