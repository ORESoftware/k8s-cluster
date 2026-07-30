//! Postgres → NATS JetStream CDC gateway.
//!
//! ## Why this exists
//!
//! Each logical-replication slot on Postgres retains WAL until its consumer
//! advances. Running one slot per service-pod multiplies both slot count
//! and "dead slot fills the disk" risk by pod count. This gateway owns ONE
//! slot (or a small leader-elected set for HA) and republishes every row
//! change to NATS JetStream where any number of consumers can subscribe
//! independently without pressuring the database.
//!
//! ## Boot sequence
//!
//! 1. Connect to Postgres.
//! 2. Verify `cdc_wal_available()` (i.e. `wal_level = logical`). Bail loudly
//!    if not — the gateway is useless without it.
//! 3. Ensure the gateway's slot exists via `cdc_ensure_wal_slot(slot, plugin)`
//!    (idempotent). The schema layer also created the `cdc_pub` publication.
//! 4. Connect to NATS, ensure the JetStream `CDC` stream covers `cdc.>`.
//! 5. Compete for the leader advisory lock (`pg_try_advisory_lock`). Only
//!    the lock holder runs the pump loop; followers idle until promoted.
//! 6. Pump loop: peek `pg_logical_slot_peek_changes(slot, …, 'wal2json',
//!    'format-version', '2', 'include-lsn', 'true')`. For each change row,
//!    publish to `cdc.<schema>.<table>.<op>` with a normalized envelope. Only
//!    after the whole peeked batch is published and acked do we advance the
//!    slot with `pg_logical_slot_get_changes`.
//!
//! ## Envelope schema (`cdc.row.v1`)
//!
//! ```json
//! {
//!   "schemaVersion": "cdc.row.v1",
//!   "schema": "public",
//!   "table": "app_config",
//!   "op": "update",
//!   "lsn": "0/1A3B5C0",
//!   "xid": 12345,
//!   "tsMs": 1736000000000,
//!   "primaryKey": ["id"],
//!   "row":          { ...new row, OR identity for delete... },
//!   "previousRow":  { ...old identity for update/delete... } | null
//! }
//! ```
//!
//! Consumers should NEVER assume the order of `row` columns matches table
//! DDL order; iterate by name. `previousRow` is null for inserts.
//!
//! ## Failure modes & guarantees
//!
//! * Slot retention: the gateway holds WAL until it has published and acked
//!   the corresponding messages to JetStream. If JetStream is unreachable
//!   the slot will accumulate. Operators must alert on
//!   `cdc_slot_lag_bytes('cdc_gateway')`.
//! * At-least-once delivery: the gateway advances the slot position AFTER
//!   every JetStream publish ack in a peeked batch returns. A crash after
//!   publish but before slot advance will redeliver. Consumers must be
//!   idempotent (key off primary key + lsn).
//! * Single-writer to the slot: the advisory lock ensures only one process
//!   reads from the slot at a time. Other replicas idle.
//!
//! ## Configuration
//!
//! See `Config` below for the full env-var matrix. Sensible defaults assume
//! the local dev cluster (`PG_DATABASE_URL`, `NATS_URL`).

mod docs;

use std::{
    collections::BTreeMap,
    env,
    error::Error,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json, Router,
};
use prometheus::{Encoder, IntCounter, IntGauge, TextEncoder};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::time::sleep;
use utoipa::openapi::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::docs::{ApiDocs, SharedApiDocs};

const SERVICE_NAME: &str = "dd-wal-gateway";
const OPENAPI_EXPORT_FLAG: &str = "--export-openapi";
const OPENAPI_CONTENT_TYPE: &str = "application/vnd.oai.openapi+json;version=3.1";
const SCHEMA_VERSION: &str = "cdc.row.v1";
// Stream name comes from the source-of-truth schema (CDC stream), so a
// rename in remote/libs/nats/subject-defs/schema/wal-cdc.schema.json
// surfaces here as a compile-time symbol break. The subject prefix is
// still operator-tunable via WAL_GATEWAY_SUBJECT_PREFIX but defaults to
// the cluster-wide "cdc" convention (every other consumer hardcodes
// "cdc" as the first token).
const DEFAULT_STREAM_NAME: &str = CDC_STREAM_NAME;
const DEFAULT_SUBJECT_PREFIX: &str = "cdc";
const DEFAULT_PORT: u16 = 8104;

/// Advisory-lock key used for leader election. `pg_try_advisory_lock` takes
/// a single bigint; we pick a deterministic 64-bit value so any replica
/// hits the same key without coordination. The number itself is arbitrary —
/// it just has to be unique across whatever else might use advisory locks
/// in the same database. Computed as the BE bytes of the ASCII string
/// "WALGATEW" so it's recognisable in `pg_locks` for ops.
const LEADER_LOCK_KEY: i64 = i64::from_be_bytes(*b"WALGATEW");

use dd_nats_subject_defs::{cdc_row_change_subject, CDC_STREAM_NAME};

#[derive(Clone)]
struct Config {
    database_url: String,
    nats_url: Option<String>,
    slot_name: String,
    plugin: String,
    stream_name: String,
    subject_prefix: String,
    poll_interval: Duration,
    publish_timeout: Duration,
    max_batch: i32,
    pod_name: String,
    http_port: u16,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let database_url = first_env(&[
            "WAL_GATEWAY_DATABASE_URL",
            "CDC_DATABASE_URL",
            "RDS_DATABASE_URL",
            "DATABASE_URL",
        ])
        .ok_or_else(|| "WAL_GATEWAY_DATABASE_URL not set".to_string())?;
        let nats_url = first_env(&["WAL_GATEWAY_NATS_URL", "NATS_URL"]);
        let slot_name = env_value("WAL_GATEWAY_SLOT_NAME", "cdc_gateway");
        let plugin = env_value("WAL_GATEWAY_PLUGIN", "wal2json");
        let stream_name = env_value("WAL_GATEWAY_STREAM_NAME", DEFAULT_STREAM_NAME);
        let subject_prefix = env_value("WAL_GATEWAY_SUBJECT_PREFIX", DEFAULT_SUBJECT_PREFIX);
        let poll_interval = Duration::from_millis(env_u64("WAL_GATEWAY_POLL_MS", 250));
        let publish_timeout = Duration::from_secs(env_u64("WAL_GATEWAY_PUBLISH_TIMEOUT_S", 5));
        let max_batch = env_u64("WAL_GATEWAY_MAX_BATCH", 2000).clamp(1, 10_000) as i32;
        let pod_name = env_value(
            "WAL_GATEWAY_POD_NAME",
            &env_value("HOSTNAME", "wal-gateway-local"),
        );
        let http_port = env_value("PORT", &DEFAULT_PORT.to_string())
            .parse()
            .map_err(|error| format!("invalid PORT: {error}"))?;
        Ok(Self {
            database_url,
            nats_url,
            slot_name,
            plugin,
            stream_name,
            subject_prefix,
            poll_interval,
            publish_timeout,
            max_batch,
            pod_name,
            http_port,
        })
    }
}

#[derive(Default)]
struct Metrics {
    started_at_ms: AtomicU64,
    leader: AtomicBool,
    polls_total: AtomicU64,
    poll_failures_total: AtomicU64,
    rows_seen_total: AtomicU64,
    rows_published_total: AtomicU64,
    publish_failures_total: AtomicU64,
    slot_advances_total: AtomicU64,
    slot_advance_failures_total: AtomicU64,
    skipped_messages_total: AtomicU64,
    last_lsn: parking_mutex::Mutex<Option<String>>,
}

/// Tiny std-only mutex wrapper so we don't pull `parking_lot` as a dep.
mod parking_mutex {
    use std::sync::Mutex as StdMutex;

    pub struct Mutex<T>(StdMutex<T>);
    impl<T: Default> Default for Mutex<T> {
        fn default() -> Self {
            Self(StdMutex::new(T::default()))
        }
    }
    impl<T: Clone> Mutex<T> {
        pub fn snapshot(&self) -> T {
            self.0.lock().expect("metrics mutex poisoned").clone()
        }
        pub fn store(&self, v: T) {
            *self.0.lock().expect("metrics mutex poisoned") = v;
        }
    }
}

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    metrics: Arc<Metrics>,
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn env_value(key: &str, fallback: &str) -> String {
    first_env(&[key]).unwrap_or_else(|| fallback.to_string())
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

fn local_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(root))
        .routes(routes!(healthz))
        .routes(routes!(readyz))
        .routes(routes!(metrics_handler))
        .routes(routes!(openapi_json))
        .routes(routes!(api_docs_json))
        .routes(routes!(api_docs_ui))
        .routes(routes!(docs_api_ui))
}

fn openapi_document() -> OpenApi {
    let (_, shared_openapi) = dd_runtime_config_client::router_and_openapi();
    docs::compose(local_router().into_openapi(), shared_openapi)
}

fn app_router(state: AppState) -> Result<Router, Box<dyn Error + Send + Sync>> {
    let (shared_router, shared_openapi) = dd_runtime_config_client::router_and_openapi();
    let (local_router, local_openapi) = local_router().split_for_parts();
    let openapi = docs::compose(local_openapi, shared_openapi);
    let api_docs = Arc::new(ApiDocs::new(&openapi)?);

    Ok(local_router
        .with_state(state)
        .merge(shared_router)
        .layer(Extension(api_docs)))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    if env::args().any(|arg| arg == OPENAPI_EXPORT_FLAG) {
        print!("{}", docs::canonical_json(&openapi_document())?);
        return Ok(());
    }

    let _otel = dd_telemetry::init("dd-wal-gateway");

    let config = Arc::new(Config::from_env().map_err(|error| {
        tracing::error!("{SERVICE_NAME} config error: {error}");
        error
    })?);
    let metrics = Arc::new(Metrics::default());
    metrics.started_at_ms.store(now_ms(), Ordering::Relaxed);

    tracing::info!(
        "{SERVICE_NAME} starting pod={} slot={} stream={} subject_prefix={} poll_ms={}",
        config.pod_name,
        config.slot_name,
        config.stream_name,
        config.subject_prefix,
        config.poll_interval.as_millis(),
    );

    let state = AppState {
        config: config.clone(),
        metrics: metrics.clone(),
    };
    let app = app_router(state)?;

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("0.0.0.0:{}", config.http_port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("{SERVICE_NAME} listening on http://{addr}");
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
        {
            tracing::error!("{SERVICE_NAME} http server error: {error}");
        }
    });

    run_gateway_forever(config, metrics).await;
    Ok(())
}

/// Outer loop: reconnect on any error, sleep a bit, keep going. The pod
/// is supervised by Kubernetes so transient PG / NATS outages are expected.
async fn run_gateway_forever(config: Arc<Config>, metrics: Arc<Metrics>) {
    loop {
        match run_gateway_once(&config, &metrics).await {
            Ok(()) => {
                tracing::error!("{SERVICE_NAME} pump exited cleanly; restarting");
            }
            Err(error) => {
                tracing::error!("{SERVICE_NAME} pump failed: {error}");
            }
        }
        metrics.leader.store(false, Ordering::Relaxed);
        sleep(Duration::from_secs(2)).await;
    }
}

async fn run_gateway_once(
    config: &Config,
    metrics: &Metrics,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    // ── 1. Postgres connection ─────────────────────────────────────────
    let pg = connect_postgres(&config.database_url).await?;

    // Smoke-test prerequisites. If WAL isn't logical there's nothing we
    // can do and we should surface the misconfiguration loudly.
    let wal_ok: bool = pg
        .query_one("select cdc_wal_available()", &[])
        .await?
        .get(0);
    if !wal_ok {
        return Err("cdc_wal_available() returned false; \
                    enable wal_level=logical on this database (rds.logical_replication=1)"
            .into());
    }
    let slot_ok: bool = pg
        .query_one(
            "select cdc_ensure_wal_slot($1::text, $2::text)",
            &[&config.slot_name, &config.plugin],
        )
        .await?
        .get(0);
    if !slot_ok {
        return Err(format!(
            "cdc_ensure_wal_slot('{}', '{}') returned false; is the '{}' output \
             plugin installed on this server?",
            config.slot_name, config.plugin, config.plugin
        )
        .into());
    }

    // ── 2. NATS / JetStream ────────────────────────────────────────────
    let Some(nats_url) = config.nats_url.as_deref() else {
        return Err("NATS_URL not configured; the gateway needs JetStream".into());
    };
    // NATS is the gateway's whole purpose, so wait for the broker on a transient
    // boot outage (retry with backoff) instead of crash-looping the pod.
    let nats = async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .connect(nats_url)
        .await?;
    let jetstream = async_nats::jetstream::new(nats.clone());
    ensure_stream(&jetstream, &config.stream_name, &config.subject_prefix).await?;

    // ── 3. Leader election ─────────────────────────────────────────────
    //
    // `pg_try_advisory_lock(key)` returns true iff the SESSION acquires
    // the lock. The lock is released when the session ends, so a leader
    // crash automatically frees the seat for a follower. We poll on a
    // dedicated short-lived connection so the main pump connection can
    // serialize transactions independently.
    let leader = wait_for_leadership(&config.database_url).await?;
    tracing::info!(
        "{SERVICE_NAME} became leader pod={} slot={}",
        config.pod_name,
        config.slot_name
    );
    metrics.leader.store(true, Ordering::Relaxed);

    // ── 4. Pump loop ──────────────────────────────────────────────────
    let pump_result = pump_loop(config, metrics, &pg, &jetstream).await;

    // Releasing the lock is implicit (session close) but the explicit
    // drop call here documents the lifetime tie.
    drop(leader);
    metrics.leader.store(false, Ordering::Relaxed);
    pump_result
}

async fn connect_postgres(database_url: &str) -> Result<tokio_postgres::Client, String> {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
    let (client, connection) = tokio_postgres::connect(database_url, tls)
        .await
        .map_err(|error| format!("postgres connect failed: {error}"))?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::error!("{SERVICE_NAME} postgres connection task ended: {error}");
        }
    });
    Ok(client)
}

/// Holds the advisory lock for as long as the connection lives. Dropping
/// the value drops the connection, which releases the lock.
struct LeadershipHandle {
    _client: tokio_postgres::Client,
}

async fn wait_for_leadership(database_url: &str) -> Result<LeadershipHandle, String> {
    loop {
        let client = connect_postgres(database_url).await?;
        let acquired: bool = client
            .query_one("select pg_try_advisory_lock($1)", &[&LEADER_LOCK_KEY])
            .await
            .map_err(|error| format!("pg_try_advisory_lock failed: {error}"))?
            .get(0);
        if acquired {
            return Ok(LeadershipHandle { _client: client });
        }
        drop(client);
        sleep(Duration::from_secs(2)).await;
    }
}

async fn ensure_stream(
    jetstream: &async_nats::jetstream::Context,
    stream_name: &str,
    subject_prefix: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    use async_nats::jetstream::stream::{Config as StreamConfig, RetentionPolicy};
    let subjects_pattern = format!("{subject_prefix}.>");
    jetstream
        .get_or_create_stream(StreamConfig {
            name: stream_name.to_string(),
            subjects: vec![subjects_pattern],
            // Limits-based retention: we don't want consumers blocking the
            // stream by failing to ack (which is what WorkQueue would do).
            // CDC is naturally redelivery-tolerant so limits-based is right.
            retention: RetentionPolicy::Limits,
            max_age: Duration::from_secs(60 * 60 * 24),
            max_messages: 10_000_000,
            ..Default::default()
        })
        .await
        .map_err(|error| format!("jetstream ensure_stream failed: {error}").into())
        .map(|_| ())
}

async fn pump_loop(
    config: &Config,
    metrics: &Metrics,
    pg: &tokio_postgres::Client,
    jetstream: &async_nats::jetstream::Context,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut interval = tokio::time::interval(config.poll_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        metrics.polls_total.fetch_add(1, Ordering::Relaxed);
        let batch = match peek_slot_changes(pg, &config.slot_name, config.max_batch).await {
            Ok(batch) => batch,
            Err(error) => {
                metrics.poll_failures_total.fetch_add(1, Ordering::Relaxed);
                tracing::error!("{SERVICE_NAME} slot peek failed: {error}");
                // Bubble up — the outer loop will reconnect everything.
                return Err(error);
            }
        };
        if batch.is_empty() {
            continue;
        }
        for raw in &batch {
            metrics.rows_seen_total.fetch_add(1, Ordering::Relaxed);
            match parse_wal2json_row(&raw.json) {
                Some(parsed) => {
                    let subject = parsed.subject(&config.subject_prefix);
                    let envelope = build_envelope(&parsed, &raw.lsn);
                    let bytes = match serde_json::to_vec(&envelope) {
                        Ok(b) => b,
                        Err(error) => {
                            tracing::error!("{SERVICE_NAME} envelope encode failed: {error}");
                            metrics
                                .publish_failures_total
                                .fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    };
                    let publish = jetstream.publish(subject, bytes.into()).await;
                    match publish {
                        Ok(ack_future) => {
                            // JetStream is async-ack: the publish call returns a
                            // future that resolves once the server has durably
                            // accepted the message. We wait with a timeout so
                            // a wedged JetStream can't lock the pump forever.
                            match tokio::time::timeout(config.publish_timeout, ack_future).await {
                                Ok(Ok(_ack)) => {
                                    metrics.rows_published_total.fetch_add(1, Ordering::Relaxed);
                                }
                                Ok(Err(error)) => {
                                    metrics
                                        .publish_failures_total
                                        .fetch_add(1, Ordering::Relaxed);
                                    tracing::error!("{SERVICE_NAME} jetstream ack failed: {error}");
                                    return Err(error.into());
                                }
                                Err(_) => {
                                    metrics
                                        .publish_failures_total
                                        .fetch_add(1, Ordering::Relaxed);
                                    return Err("jetstream publish timed out".into());
                                }
                            }
                        }
                        Err(error) => {
                            metrics
                                .publish_failures_total
                                .fetch_add(1, Ordering::Relaxed);
                            tracing::error!("{SERVICE_NAME} jetstream publish failed: {error}");
                            return Err(error.into());
                        }
                    }
                }
                None => {
                    metrics
                        .skipped_messages_total
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            metrics.last_lsn.store(Some(raw.lsn.clone()));
        }
        match advance_slot_changes(pg, &config.slot_name, batch.len() as i32).await {
            Ok(advanced) => {
                metrics.slot_advances_total.fetch_add(1, Ordering::Relaxed);
                if advanced != batch.len() {
                    return Err(format!(
                        "slot advance consumed {advanced} messages after peeking {}; \
                         refusing to continue because the slot cursor may be misaligned",
                        batch.len()
                    )
                    .into());
                }
            }
            Err(error) => {
                metrics
                    .slot_advance_failures_total
                    .fetch_add(1, Ordering::Relaxed);
                tracing::error!("{SERVICE_NAME} slot advance failed: {error}");
                return Err(error);
            }
        }
    }
}

struct SlotRow {
    lsn: String,
    #[allow(dead_code)]
    xid: i64,
    json: String,
}

async fn peek_slot_changes(
    pg: &tokio_postgres::Client,
    slot_name: &str,
    upto_nchanges: i32,
) -> Result<Vec<SlotRow>, Box<dyn Error + Send + Sync>> {
    // Never read with `_get_changes` before publishing. `_get_changes`
    // advances the logical slot when the SQL statement commits, which can
    // happen before our application has durably published the messages to
    // JetStream. Peeking first keeps Postgres' slot cursor parked until the
    // whole batch is published and acked.
    let rows = pg
        .query(
            "select lsn::text, xid::text, data
             from pg_logical_slot_peek_changes(
               $1::text, null, $2::int,
               'format-version', '2',
               'include-lsn', 'true',
               'include-xids', 'true',
               'include-timestamp', 'true',
               'include-types', 'false'
             )",
            &[&slot_name, &upto_nchanges],
        )
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let lsn: String = row.get(0);
            let xid_text: String = row.get(1);
            let json: String = row.get(2);
            // wal2json emits xid as a number in the JSON body too; the
            // SQL projection gives us it as text already, so we just
            // parse defensively and fall through if it's something
            // unexpected.
            let xid = xid_text.parse::<i64>().unwrap_or(0);
            SlotRow { lsn, xid, json }
        })
        .collect())
}

async fn advance_slot_changes(
    pg: &tokio_postgres::Client,
    slot_name: &str,
    nchanges: i32,
) -> Result<usize, Box<dyn Error + Send + Sync>> {
    if nchanges <= 0 {
        return Ok(0);
    }
    // Consume exactly the number of plugin messages we just peeked. If the
    // process crashes or this query fails after JetStream accepted the
    // messages, the batch will be redelivered on restart; that is the intended
    // at-least-once duplicate, and consumers key off `(table, primary_key, lsn)`.
    let rows = pg
        .query(
            "select 1
             from pg_logical_slot_get_changes(
               $1::text, null, $2::int,
               'format-version', '2',
               'include-lsn', 'true',
               'include-xids', 'true',
               'include-timestamp', 'true',
               'include-types', 'false'
             )",
            &[&slot_name, &nchanges],
        )
        .await?;
    Ok(rows.len())
}

#[derive(Debug)]
struct ParsedChange {
    schema: String,
    table: String,
    op: ChangeOp,
    xid: Option<i64>,
    timestamp: Option<String>,
    columns: BTreeMap<String, Value>,
    identity: BTreeMap<String, Value>,
    pk_names: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
enum ChangeOp {
    Insert,
    Update,
    Delete,
}

impl ChangeOp {
    fn as_str(self) -> &'static str {
        match self {
            ChangeOp::Insert => "insert",
            ChangeOp::Update => "update",
            ChangeOp::Delete => "delete",
        }
    }
    fn from_action(action: &str) -> Option<Self> {
        match action {
            "I" => Some(ChangeOp::Insert),
            "U" => Some(ChangeOp::Update),
            "D" => Some(ChangeOp::Delete),
            _ => None,
        }
    }
}

impl ParsedChange {
    fn subject(&self, prefix: &str) -> String {
        cdc_row_change_subject(prefix, &self.schema, &self.table, self.op.as_str())
    }
}

/// Parse a single wal2json format-version 2 line.
///
/// Returns `None` for BEGIN / COMMIT / TRUNCATE / MESSAGE envelopes, which
/// we deliberately drop — consumers only care about row-level changes.
fn parse_wal2json_row(json_line: &str) -> Option<ParsedChange> {
    let value: Value = serde_json::from_str(json_line).ok()?;
    let obj = value.as_object()?;
    let action = obj.get("action").and_then(Value::as_str)?;
    let op = ChangeOp::from_action(action)?;
    let schema = obj
        .get("schema")
        .and_then(Value::as_str)
        .unwrap_or("public")
        .to_string();
    let table = obj.get("table").and_then(Value::as_str)?.to_string();
    let xid = obj
        .get("xid")
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()));
    let timestamp = obj
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut columns = BTreeMap::new();
    let mut pk_names = Vec::new();
    if let Some(items) = obj.get("columns").and_then(Value::as_array) {
        for item in items {
            if let (Some(name), Some(value)) =
                (item.get("name").and_then(Value::as_str), item.get("value"))
            {
                columns.insert(name.to_string(), value.clone());
            }
        }
    }
    let mut identity = BTreeMap::new();
    if let Some(items) = obj.get("identity").and_then(Value::as_array) {
        for item in items {
            if let (Some(name), Some(value)) =
                (item.get("name").and_then(Value::as_str), item.get("value"))
            {
                identity.insert(name.to_string(), value.clone());
                pk_names.push(name.to_string());
            }
        }
    }
    // For INSERT wal2json does not emit an `identity` array; treat the full
    // column set as the PK source if the publication's REPLICA IDENTITY is
    // FULL or DEFAULT. We approximate by leaving pk_names empty; consumers
    // that need PK lookup can use `identity` directly for U/D and fall back
    // to a known PK name (usually `id`) for I.
    Some(ParsedChange {
        schema,
        table,
        op,
        xid,
        timestamp,
        columns,
        identity,
        pk_names,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<'a> {
    schema_version: &'static str,
    schema: &'a str,
    table: &'a str,
    op: &'static str,
    lsn: &'a str,
    xid: Option<i64>,
    ts_ms: u64,
    source_timestamp: Option<&'a str>,
    primary_key: &'a [String],
    row: Value,
    previous_row: Option<Value>,
}

fn build_envelope<'a>(parsed: &'a ParsedChange, lsn: &'a str) -> Envelope<'a> {
    let row = match parsed.op {
        ChangeOp::Insert | ChangeOp::Update => {
            // INSERT and UPDATE both carry the full column set in `columns`.
            // Fall back to identity if the publication is column-list-
            // restricted and `columns` is empty.
            if parsed.columns.is_empty() {
                Value::Object(parsed.identity.clone().into_iter().collect())
            } else {
                Value::Object(parsed.columns.clone().into_iter().collect())
            }
        }
        ChangeOp::Delete => Value::Object(parsed.identity.clone().into_iter().collect()),
    };
    let previous_row = match parsed.op {
        ChangeOp::Update | ChangeOp::Delete => {
            Some(Value::Object(parsed.identity.clone().into_iter().collect()))
        }
        ChangeOp::Insert => None,
    };
    Envelope {
        schema_version: SCHEMA_VERSION,
        schema: &parsed.schema,
        table: &parsed.table,
        op: parsed.op.as_str(),
        lsn,
        xid: parsed.xid,
        ts_ms: now_ms(),
        source_timestamp: parsed.timestamp.as_deref(),
        primary_key: &parsed.pk_names,
        row,
        previous_row,
    }
}

// ── HTTP handlers ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct RootResponse {
    ok: bool,
    service: &'static str,
    schema_version: &'static str,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    service: &'static str,
    leader: bool,
    polls: u64,
    rows_published: u64,
    at_ms: u64,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct ReadinessResponse {
    ok: bool,
    service: &'static str,
    leader: bool,
    at_ms: u64,
}

#[utoipa::path(
    get,
    path = "/",
    operation_id = "getWalGatewayDescriptor",
    tag = "service",
    security(()),
    responses((status = 200, description = "Stable public service descriptor", body = RootResponse))
)]
async fn root() -> impl IntoResponse {
    Json(RootResponse {
        ok: true,
        service: SERVICE_NAME,
        schema_version: SCHEMA_VERSION,
    })
}

#[utoipa::path(
    get,
    path = "/healthz",
    operation_id = "getWalGatewayHealth",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Process health and bounded internal counters", body = HealthResponse))
)]
async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    Json(HealthResponse {
        ok: true,
        service: SERVICE_NAME,
        leader: state.metrics.leader.load(Ordering::Relaxed),
        polls: state.metrics.polls_total.load(Ordering::Relaxed),
        rows_published: state.metrics.rows_published_total.load(Ordering::Relaxed),
        at_ms: now_ms(),
    })
}

#[utoipa::path(
    get,
    path = "/readyz",
    operation_id = "getWalGatewayReadiness",
    tag = "operations",
    security(()),
    responses(
        (status = 200, description = "Gateway is configured to reach JetStream", body = ReadinessResponse),
        (status = 503, description = "A required gateway dependency is not configured", body = ReadinessResponse)
    )
)]
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    // Followers are ready (they're correctly idle waiting for the lock);
    // only "not configured" is unready.
    let ok = state.config.nats_url.is_some();
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(ReadinessResponse {
            ok,
            service: SERVICE_NAME,
            leader: state.metrics.leader.load(Ordering::Relaxed),
            at_ms: now_ms(),
        }),
    )
}

#[utoipa::path(
    get,
    path = "/metrics",
    operation_id = "getWalGatewayPrometheusMetrics",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Prometheus text exposition", body = String, content_type = "text/plain"))
)]
async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    // Build a minimal Prometheus exposition text manually; we use the
    // prometheus crate only for the encoder type, to stay consistent with
    // other rust services in the workspace.
    let registry = prometheus::Registry::new();
    macro_rules! gauge {
        ($name:expr, $help:expr, $value:expr) => {{
            let g = IntGauge::new($name, $help).unwrap();
            g.set($value as i64);
            registry.register(Box::new(g)).unwrap();
        }};
    }
    macro_rules! counter {
        ($name:expr, $help:expr, $value:expr) => {{
            let c = IntCounter::new($name, $help).unwrap();
            c.inc_by($value);
            registry.register(Box::new(c)).unwrap();
        }};
    }
    gauge!(
        "dd_wal_gateway_is_leader",
        "1 if this replica currently holds the slot lock.",
        state.metrics.leader.load(Ordering::Relaxed) as i64
    );
    counter!(
        "dd_wal_gateway_polls_total",
        "Slot polls executed.",
        state.metrics.polls_total.load(Ordering::Relaxed)
    );
    counter!(
        "dd_wal_gateway_poll_failures_total",
        "Slot polls that returned an error.",
        state.metrics.poll_failures_total.load(Ordering::Relaxed)
    );
    counter!(
        "dd_wal_gateway_rows_seen_total",
        "Row changes received from the slot (including skipped).",
        state.metrics.rows_seen_total.load(Ordering::Relaxed)
    );
    counter!(
        "dd_wal_gateway_rows_published_total",
        "Row changes successfully published to JetStream.",
        state.metrics.rows_published_total.load(Ordering::Relaxed)
    );
    counter!(
        "dd_wal_gateway_publish_failures_total",
        "Row publishes that failed or timed out.",
        state.metrics.publish_failures_total.load(Ordering::Relaxed)
    );
    counter!(
        "dd_wal_gateway_slot_advances_total",
        "Successful logical slot advance calls.",
        state.metrics.slot_advances_total.load(Ordering::Relaxed)
    );
    counter!(
        "dd_wal_gateway_slot_advance_failures_total",
        "Logical slot advance calls that failed after a peeked batch.",
        state
            .metrics
            .slot_advance_failures_total
            .load(Ordering::Relaxed)
    );
    counter!(
        "dd_wal_gateway_skipped_messages_total",
        "Slot messages skipped (BEGIN/COMMIT/etc).",
        state.metrics.skipped_messages_total.load(Ordering::Relaxed)
    );
    let mut buffer = Vec::new();
    let encoder = TextEncoder::new();
    let metric_families = registry.gather();
    if let Err(error) = encoder.encode(&metric_families, &mut buffer) {
        tracing::error!("{SERVICE_NAME} metrics encode failed: {error}");
    }
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        buffer,
    )
}

#[utoipa::path(
    get,
    path = "/openapi.json",
    operation_id = "getWalGatewayPublicOpenApiDocument",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Fail-closed public OpenAPI 3.1 contract", content_type = "application/vnd.oai.openapi+json;version=3.1"))
)]
async fn openapi_json(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_openapi_response(docs)
}

#[utoipa::path(
    get,
    path = "/api/docs.json",
    operation_id = "getWalGatewayPublicOpenApiDocumentCompatibilityAlias",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Compatibility alias for the fail-closed public OpenAPI 3.1 contract", content_type = "application/vnd.oai.openapi+json;version=3.1"))
)]
async fn api_docs_json(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_openapi_response(docs)
}

#[utoipa::path(
    get,
    path = "/api/docs",
    operation_id = "getWalGatewayPublicApiReference",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Interactive Scalar reference for the fail-closed public contract", body = String, content_type = "text/html"))
)]
async fn api_docs_ui(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_scalar_response(docs)
}

#[utoipa::path(
    get,
    path = "/docs/api",
    operation_id = "getWalGatewayPublicApiReferenceCompatibilityAlias",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Compatibility alias for the public Scalar API reference", body = String, content_type = "text/html"))
)]
async fn docs_api_ui(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_scalar_response(docs)
}

fn public_openapi_response(docs: SharedApiDocs) -> Response {
    (
        [(header::CONTENT_TYPE, OPENAPI_CONTENT_TYPE)],
        docs.public_json.clone(),
    )
        .into_response()
}

fn public_scalar_response(docs: SharedApiDocs) -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        docs.public_scalar_html.clone(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    const EXPECTED_OPENAPI_PATHS: &[&str] = &[
        "/",
        "/api/docs",
        "/api/docs.json",
        "/docs/api",
        "/healthz",
        "/internal/runtime-config",
        "/internal/runtime-config/reset",
        "/internal/update-runtime-config",
        "/metrics",
        "/openapi.json",
        "/readyz",
    ];

    // ── env-var test scaffolding ──────────────────────────────────────────
    //
    // `Config::from_env` and the `env_*` helpers read process-global state,
    // which the test harness exercises across threads. Every env-touching test
    // serializes on one lock and fully restores the prior environment before
    // returning, so the tests are hermetic and order-independent. Assertions
    // are always made on the value RETURNED from `with_env` (outside the lock),
    // never inside the closure, so a failed assert can never leak a mutated
    // environment into another test.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Every env var `Config::from_env` consults. `config_with` clears all of
    /// them before applying overrides so the developer's shell (HOSTNAME,
    /// DATABASE_URL, …) can't perturb a defaults-oriented assertion.
    const ALL_CONFIG_KEYS: &[&str] = &[
        "WAL_GATEWAY_DATABASE_URL",
        "CDC_DATABASE_URL",
        "RDS_DATABASE_URL",
        "DATABASE_URL",
        "WAL_GATEWAY_NATS_URL",
        "NATS_URL",
        "WAL_GATEWAY_SLOT_NAME",
        "WAL_GATEWAY_PLUGIN",
        "WAL_GATEWAY_STREAM_NAME",
        "WAL_GATEWAY_SUBJECT_PREFIX",
        "WAL_GATEWAY_POLL_MS",
        "WAL_GATEWAY_PUBLISH_TIMEOUT_S",
        "WAL_GATEWAY_MAX_BATCH",
        "WAL_GATEWAY_POD_NAME",
        "HOSTNAME",
        "PORT",
    ];

    /// Run `f` with a controlled environment, restoring the prior values of
    /// exactly the touched keys afterwards. `Some(v)` sets a key, `None` clears
    /// it. All mutation is serialized on `ENV_LOCK` and reverted before return.
    fn with_env<R>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> R) -> R {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(key, _)| ((*key).to_string(), env::var(key).ok()))
            .collect();
        for (key, value) in vars {
            match value {
                Some(v) => env::set_var(key, v),
                None => env::remove_var(key),
            }
        }
        let out = f();
        for (key, prior) in saved {
            match prior {
                Some(v) => env::set_var(&key, v),
                None => env::remove_var(&key),
            }
        }
        out
    }

    /// Clear every config key, apply `overrides`, then run `Config::from_env`.
    fn config_with(overrides: &[(&str, &str)]) -> Result<Config, String> {
        let mut vars: Vec<(&str, Option<&str>)> =
            ALL_CONFIG_KEYS.iter().map(|key| (*key, None)).collect();
        for (key, value) in overrides {
            vars.push((key, Some(value)));
        }
        with_env(&vars, Config::from_env)
    }

    #[test]
    fn parses_wal2json_insert() {
        let line = r#"{
          "action":"I",
          "schema":"public",
          "table":"app_config",
          "xid":12345,
          "timestamp":"2025-01-01 00:00:00+00",
          "columns":[
            {"name":"id","type":"uuid","value":"00000000-0000-0000-0000-000000000001"},
            {"name":"scope","type":"varchar","value":"default"},
            {"name":"key","type":"varchar","value":"trading.platforms.v1"}
          ]
        }"#;
        let parsed = parse_wal2json_row(line).expect("parsed");
        assert_eq!(parsed.table, "app_config");
        assert!(matches!(parsed.op, ChangeOp::Insert));
        assert_eq!(parsed.columns.get("scope").unwrap(), "default");
        assert_eq!(parsed.subject("cdc"), "cdc.public.app_config.insert");
    }

    #[test]
    fn parses_wal2json_update_with_identity() {
        let line = r#"{
          "action":"U",
          "schema":"public",
          "table":"container_pool_configs",
          "columns":[
            {"name":"id","type":"uuid","value":"00000000-0000-0000-0000-000000000002"},
            {"name":"min_warm","type":"integer","value":3}
          ],
          "identity":[
            {"name":"id","type":"uuid","value":"00000000-0000-0000-0000-000000000002"}
          ]
        }"#;
        let parsed = parse_wal2json_row(line).expect("parsed");
        assert!(matches!(parsed.op, ChangeOp::Update));
        assert_eq!(parsed.pk_names, vec!["id"]);
        let env = build_envelope(&parsed, "0/1A3B5C0");
        assert!(env.previous_row.is_some());
        assert_eq!(env.row.get("min_warm").unwrap(), 3);
    }

    #[test]
    fn parses_wal2json_delete_uses_identity_only() {
        let line = r#"{
          "action":"D",
          "schema":"public",
          "table":"lambda_functions",
          "identity":[
            {"name":"id","type":"uuid","value":"00000000-0000-0000-0000-000000000003"}
          ]
        }"#;
        let parsed = parse_wal2json_row(line).expect("parsed");
        let env = build_envelope(&parsed, "0/1A3B5C0");
        assert_eq!(env.op, "delete");
        assert!(env.row.get("id").is_some());
        assert!(env.previous_row.is_some());
    }

    #[test]
    fn drops_begin_commit_envelopes() {
        assert!(parse_wal2json_row(r#"{"action":"B"}"#).is_none());
        assert!(parse_wal2json_row(r#"{"action":"C"}"#).is_none());
        assert!(parse_wal2json_row("not even json").is_none());
    }

    // ── Config: defaults, overrides, precedence, clamping ─────────────────

    #[test]
    fn config_defaults_are_deterministic() {
        let cfg = config_with(&[("WAL_GATEWAY_DATABASE_URL", "postgres://u@h/db")])
            .expect("config should build with only a database url");
        assert_eq!(cfg.database_url, "postgres://u@h/db");
        assert!(cfg.nats_url.is_none());
        assert_eq!(cfg.slot_name, "cdc_gateway");
        assert_eq!(cfg.plugin, "wal2json");
        assert_eq!(cfg.stream_name, "CDC");
        assert_eq!(cfg.subject_prefix, "cdc");
        assert_eq!(cfg.poll_interval, Duration::from_millis(250));
        assert_eq!(cfg.publish_timeout, Duration::from_secs(5));
        assert_eq!(cfg.max_batch, 2000);
        assert_eq!(cfg.pod_name, "wal-gateway-local");
        assert_eq!(cfg.http_port, 8104);
    }

    #[test]
    fn config_missing_database_url_errors() {
        // (`Config` intentionally derives no Debug, so map Ok -> () before
        //  expect_err rather than requiring Config: Debug.)
        let err = config_with(&[])
            .map(|_| ())
            .expect_err("no database url must error");
        assert!(
            err.contains("WAL_GATEWAY_DATABASE_URL not set"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn config_database_url_precedence() {
        // Lowest-precedence DATABASE_URL is used when it's the only one set.
        assert_eq!(
            config_with(&[("DATABASE_URL", "url-database")])
                .unwrap()
                .database_url,
            "url-database"
        );
        // CDC_DATABASE_URL beats DATABASE_URL.
        assert_eq!(
            config_with(&[
                ("CDC_DATABASE_URL", "url-cdc"),
                ("DATABASE_URL", "url-database"),
            ])
            .unwrap()
            .database_url,
            "url-cdc"
        );
        // RDS_DATABASE_URL beats DATABASE_URL.
        assert_eq!(
            config_with(&[
                ("RDS_DATABASE_URL", "url-rds"),
                ("DATABASE_URL", "url-database"),
            ])
            .unwrap()
            .database_url,
            "url-rds"
        );
        // WAL_GATEWAY_DATABASE_URL wins over everything.
        assert_eq!(
            config_with(&[
                ("WAL_GATEWAY_DATABASE_URL", "url-wal"),
                ("CDC_DATABASE_URL", "url-cdc"),
                ("RDS_DATABASE_URL", "url-rds"),
                ("DATABASE_URL", "url-database"),
            ])
            .unwrap()
            .database_url,
            "url-wal"
        );
    }

    #[test]
    fn config_nats_url_is_optional_and_ordered() {
        assert!(config_with(&[("WAL_GATEWAY_DATABASE_URL", "db")])
            .unwrap()
            .nats_url
            .is_none());
        assert_eq!(
            config_with(&[
                ("WAL_GATEWAY_DATABASE_URL", "db"),
                ("NATS_URL", "nats://plain"),
            ])
            .unwrap()
            .nats_url
            .as_deref(),
            Some("nats://plain")
        );
        // Service-specific override wins over the generic NATS_URL.
        assert_eq!(
            config_with(&[
                ("WAL_GATEWAY_DATABASE_URL", "db"),
                ("WAL_GATEWAY_NATS_URL", "nats://wal"),
                ("NATS_URL", "nats://plain"),
            ])
            .unwrap()
            .nats_url
            .as_deref(),
            Some("nats://wal")
        );
    }

    #[test]
    fn config_explicit_overrides_applied() {
        let cfg = config_with(&[
            ("WAL_GATEWAY_DATABASE_URL", "db"),
            ("WAL_GATEWAY_SLOT_NAME", "my_slot"),
            ("WAL_GATEWAY_PLUGIN", "test_decoding"),
            ("WAL_GATEWAY_STREAM_NAME", "MYSTREAM"),
            ("WAL_GATEWAY_SUBJECT_PREFIX", "myprefix"),
            ("WAL_GATEWAY_POLL_MS", "1000"),
            ("WAL_GATEWAY_PUBLISH_TIMEOUT_S", "30"),
            ("WAL_GATEWAY_POD_NAME", "pod-7"),
            ("PORT", "9090"),
        ])
        .unwrap();
        assert_eq!(cfg.slot_name, "my_slot");
        assert_eq!(cfg.plugin, "test_decoding");
        assert_eq!(cfg.stream_name, "MYSTREAM");
        assert_eq!(cfg.subject_prefix, "myprefix");
        assert_eq!(cfg.poll_interval, Duration::from_millis(1000));
        assert_eq!(cfg.publish_timeout, Duration::from_secs(30));
        assert_eq!(cfg.pod_name, "pod-7");
        assert_eq!(cfg.http_port, 9090);
    }

    #[test]
    fn config_max_batch_is_clamped_and_zero_falls_back() {
        let batch = |v: &str| {
            config_with(&[
                ("WAL_GATEWAY_DATABASE_URL", "db"),
                ("WAL_GATEWAY_MAX_BATCH", v),
            ])
            .unwrap()
            .max_batch
        };
        assert_eq!(batch("5"), 5); // in-range passes through
        assert_eq!(batch("50000"), 10_000); // above ceiling clamps down
                                            // NOTE current behavior: env_u64 discards 0 (its `> 0` filter) BEFORE
                                            // the clamp, so 0 becomes the 2000 default rather than clamping to 1.
        assert_eq!(batch("0"), 2000);
        assert_eq!(batch("banana"), 2000); // non-numeric -> default
    }

    #[test]
    fn config_zero_or_invalid_durations_fall_back() {
        // 0 is filtered by env_u64's `> 0` guard, so it maps to the default
        // rather than a zero-length interval (which would hot-spin the pump).
        assert_eq!(
            config_with(&[
                ("WAL_GATEWAY_DATABASE_URL", "db"),
                ("WAL_GATEWAY_POLL_MS", "0")
            ])
            .unwrap()
            .poll_interval,
            Duration::from_millis(250)
        );
        assert_eq!(
            config_with(&[
                ("WAL_GATEWAY_DATABASE_URL", "db"),
                ("WAL_GATEWAY_PUBLISH_TIMEOUT_S", "soon"),
            ])
            .unwrap()
            .publish_timeout,
            Duration::from_secs(5)
        );
    }

    #[test]
    fn config_pod_name_fallback_chain() {
        // HOSTNAME is used when the explicit pod name is unset.
        assert_eq!(
            config_with(&[("WAL_GATEWAY_DATABASE_URL", "db"), ("HOSTNAME", "host-9")])
                .unwrap()
                .pod_name,
            "host-9"
        );
        // Explicit pod name beats HOSTNAME.
        assert_eq!(
            config_with(&[
                ("WAL_GATEWAY_DATABASE_URL", "db"),
                ("HOSTNAME", "host-9"),
                ("WAL_GATEWAY_POD_NAME", "explicit"),
            ])
            .unwrap()
            .pod_name,
            "explicit"
        );
        // Neither present -> static default.
        assert_eq!(
            config_with(&[("WAL_GATEWAY_DATABASE_URL", "db")])
                .unwrap()
                .pod_name,
            "wal-gateway-local"
        );
    }

    #[test]
    fn config_invalid_port_errors() {
        let err = config_with(&[("WAL_GATEWAY_DATABASE_URL", "db"), ("PORT", "not-a-port")])
            .map(|_| ())
            .expect_err("bad PORT must error");
        assert!(err.contains("invalid PORT"), "unexpected error: {err}");
    }

    // ── env helpers ───────────────────────────────────────────────────────

    #[test]
    fn env_u64_filters_zero_negative_and_nonnumeric() {
        let k = "WALGW_TEST_U64";
        assert_eq!(with_env(&[(k, None)], || env_u64(k, 7)), 7); // unset -> fallback
        assert_eq!(with_env(&[(k, Some("0"))], || env_u64(k, 7)), 7); // 0 filtered out
        assert_eq!(with_env(&[(k, Some("  42 "))], || env_u64(k, 7)), 42); // trims whitespace
        assert_eq!(with_env(&[(k, Some("-1"))], || env_u64(k, 7)), 7); // negative fails parse
        assert_eq!(with_env(&[(k, Some("nope"))], || env_u64(k, 7)), 7); // non-numeric
        assert_eq!(
            with_env(&[(k, Some("99999999999999999999999999"))], || env_u64(k, 7)),
            7 // overflows u64 -> fallback
        );
    }

    #[test]
    fn first_env_trims_skips_blank_and_is_ordered() {
        let a = "WALGW_TEST_A";
        let b = "WALGW_TEST_B";
        assert_eq!(
            with_env(&[(a, None), (b, Some("  hello "))], || first_env(&[a, b])),
            Some("hello".to_string())
        );
        // A blank/whitespace-only value is treated as absent; fall through.
        assert_eq!(
            with_env(&[(a, Some("   ")), (b, Some("real"))], || first_env(&[
                a, b
            ])),
            Some("real".to_string())
        );
        // First present key wins.
        assert_eq!(
            with_env(&[(a, Some("x")), (b, Some("y"))], || first_env(&[a, b])),
            Some("x".to_string())
        );
        assert_eq!(
            with_env(&[(a, None), (b, None)], || first_env(&[a, b])),
            None
        );
    }

    #[test]
    fn env_value_falls_back_on_missing_or_blank() {
        let k = "WALGW_TEST_VAL";
        assert_eq!(with_env(&[(k, None)], || env_value(k, "fb")), "fb");
        assert_eq!(with_env(&[(k, Some("   "))], || env_value(k, "fb")), "fb"); // blank -> fallback
        assert_eq!(with_env(&[(k, Some("  v "))], || env_value(k, "fb")), "v"); // trimmed
    }

    // ── ChangeOp mapping ──────────────────────────────────────────────────

    #[test]
    fn change_op_action_mapping_is_exact() {
        assert!(matches!(ChangeOp::from_action("I"), Some(ChangeOp::Insert)));
        assert!(matches!(ChangeOp::from_action("U"), Some(ChangeOp::Update)));
        assert!(matches!(ChangeOp::from_action("D"), Some(ChangeOp::Delete)));
        for bad in ["B", "C", "T", "M", "i", "insert", "", "id"] {
            assert!(
                ChangeOp::from_action(bad).is_none(),
                "expected None for action {bad:?}"
            );
        }
        assert_eq!(ChangeOp::Insert.as_str(), "insert");
        assert_eq!(ChangeOp::Update.as_str(), "update");
        assert_eq!(ChangeOp::Delete.as_str(), "delete");
        // `from_action` speaks wal2json codes, `as_str` speaks envelope words;
        // they are deliberately NOT inverses.
        for op in [ChangeOp::Insert, ChangeOp::Update, ChangeOp::Delete] {
            assert!(ChangeOp::from_action(op.as_str()).is_none());
        }
    }

    // ── Decode: parse_wal2json_row edge cases ─────────────────────────────

    #[test]
    fn parse_insert_has_empty_pk_and_no_previous_row() {
        let line =
            r#"{"action":"I","schema":"public","table":"t","columns":[{"name":"id","value":1}]}"#;
        let parsed = parse_wal2json_row(line).expect("parsed");
        assert!(parsed.pk_names.is_empty());
        assert!(parsed.identity.is_empty());
        let env = build_envelope(&parsed, "0/1");
        assert!(env.previous_row.is_none());
        assert!(env.primary_key.is_empty());
        assert_eq!(env.row.get("id").unwrap(), 1);
    }

    #[test]
    fn parse_rejects_control_and_unknown_actions() {
        for action in ["B", "C", "T", "M", "X", "", "u"] {
            let line = format!(r#"{{"action":"{action}","table":"t"}}"#);
            assert!(
                parse_wal2json_row(&line).is_none(),
                "expected drop for action {action:?}"
            );
        }
    }

    #[test]
    fn parse_recovers_from_malformed_truncated_and_non_object() {
        assert!(parse_wal2json_row("").is_none());
        assert!(parse_wal2json_row("not even json").is_none());
        // A record truncated mid-token at the stream tail must yield None,
        // never panic (the pump skips it and advances).
        let truncated = r#"{"action":"I","table":"t","columns":[{"name":"a","valu"#;
        assert!(parse_wal2json_row(truncated).is_none());
        // Valid JSON that isn't a row object.
        assert!(parse_wal2json_row("[1,2,3]").is_none());
        assert!(parse_wal2json_row("42").is_none());
        assert!(parse_wal2json_row("null").is_none());
        assert!(parse_wal2json_row("\"just a string\"").is_none());
        // Objects missing required fields.
        assert!(parse_wal2json_row(r#"{"schema":"public"}"#).is_none()); // no action
        assert!(parse_wal2json_row(r#"{"action":"I","schema":"public"}"#).is_none());
        // no table
    }

    #[test]
    fn parse_defaults_schema_to_public() {
        let line = r#"{"action":"I","table":"t","columns":[{"name":"id","value":1}]}"#;
        assert_eq!(parse_wal2json_row(line).unwrap().schema, "public");
        // A non-string schema also falls back to "public".
        let line2 =
            r#"{"action":"I","schema":123,"table":"t","columns":[{"name":"id","value":1}]}"#;
        assert_eq!(parse_wal2json_row(line2).unwrap().schema, "public");
    }

    #[test]
    fn parse_reads_xid_as_number_or_string() {
        let num = r#"{"action":"I","table":"t","xid":12345,"columns":[{"name":"id","value":1}]}"#;
        assert_eq!(parse_wal2json_row(num).unwrap().xid, Some(12345));
        let string =
            r#"{"action":"I","table":"t","xid":"67890","columns":[{"name":"id","value":1}]}"#;
        assert_eq!(parse_wal2json_row(string).unwrap().xid, Some(67890));
        let missing = r#"{"action":"I","table":"t","columns":[{"name":"id","value":1}]}"#;
        assert_eq!(parse_wal2json_row(missing).unwrap().xid, None);
        // A non-numeric xid string parses to None rather than erroring the row.
        let bad = r#"{"action":"I","table":"t","xid":"abc","columns":[{"name":"id","value":1}]}"#;
        assert_eq!(parse_wal2json_row(bad).unwrap().xid, None);
    }

    #[test]
    fn parse_columns_skip_missing_value_but_keep_json_null() {
        let line = r#"{"action":"I","table":"t","columns":[
            {"name":"a","value":1},
            {"name":"nokeyval"},
            {"name":"c","value":null},
            {"value":99}
        ]}"#;
        let parsed = parse_wal2json_row(line).expect("parsed");
        assert_eq!(parsed.columns.get("a").unwrap(), 1);
        // Missing "value" key -> the column is dropped entirely.
        assert!(!parsed.columns.contains_key("nokeyval"));
        // Explicit JSON null is preserved (distinct from a missing column).
        assert_eq!(parsed.columns.get("c").unwrap(), &Value::Null);
        // Entry with no "name" is dropped, so only "a" and "c" survive.
        assert_eq!(parsed.columns.len(), 2);
    }

    #[test]
    fn parse_duplicate_columns_last_write_wins() {
        let line = r#"{"action":"I","table":"t","columns":[{"name":"k","value":1},{"name":"k","value":2}]}"#;
        let parsed = parse_wal2json_row(line).expect("parsed");
        assert_eq!(parsed.columns.len(), 1);
        assert_eq!(parsed.columns.get("k").unwrap(), 2);
    }

    #[test]
    fn parse_requires_string_table() {
        assert!(parse_wal2json_row(r#"{"action":"I","table":123}"#).is_none());
        assert!(parse_wal2json_row(r#"{"action":"I","table":null}"#).is_none());
    }

    // ── Encode: build_envelope + wire serialization ───────────────────────

    #[test]
    fn envelope_insert_shape() {
        let line = r#"{"action":"I","schema":"public","table":"orders","xid":7,"timestamp":"2025-01-01 00:00:00+00","columns":[{"name":"id","value":10},{"name":"amt","value":99}]}"#;
        let parsed = parse_wal2json_row(line).unwrap();
        let env = build_envelope(&parsed, "0/ABC");
        assert_eq!(env.schema_version, SCHEMA_VERSION);
        assert_eq!(env.schema_version, "cdc.row.v1");
        assert_eq!(env.schema, "public");
        assert_eq!(env.table, "orders");
        assert_eq!(env.op, "insert");
        assert_eq!(env.lsn, "0/ABC");
        assert_eq!(env.xid, Some(7));
        assert_eq!(env.source_timestamp, Some("2025-01-01 00:00:00+00"));
        assert_eq!(env.row.get("amt").unwrap(), 99);
        assert!(env.previous_row.is_none());
        assert!(env.primary_key.is_empty());
    }

    #[test]
    fn envelope_update_row_is_new_and_previous_is_identity() {
        let line = r#"{"action":"U","schema":"public","table":"t","columns":[{"name":"id","value":1},{"name":"v","value":"new"}],"identity":[{"name":"id","value":1}]}"#;
        let parsed = parse_wal2json_row(line).unwrap();
        let env = build_envelope(&parsed, "0/1");
        // `row` carries the NEW tuple (has the changed column)...
        assert_eq!(env.row.get("v").unwrap(), "new");
        // ...while `previousRow` is only the identity/PK image and lacks it.
        let prev = env.previous_row.as_ref().unwrap();
        assert_eq!(prev.get("id").unwrap(), 1);
        assert!(prev.get("v").is_none());
        assert_eq!(env.primary_key.to_vec(), vec!["id".to_string()]);
    }

    #[test]
    fn envelope_delete_row_and_previous_are_both_identity() {
        let line =
            r#"{"action":"D","schema":"public","table":"t","identity":[{"name":"id","value":42}]}"#;
        let parsed = parse_wal2json_row(line).unwrap();
        let env = build_envelope(&parsed, "0/1");
        assert_eq!(env.op, "delete");
        assert_eq!(env.row.get("id").unwrap(), 42);
        assert_eq!(env.previous_row.as_ref().unwrap().get("id").unwrap(), 42);
        // For deletes, row and previousRow are the same identity image.
        assert_eq!(env.row, env.previous_row.clone().unwrap());
    }

    #[test]
    fn envelope_empty_columns_fall_back_to_identity() {
        // Update with empty columns falls back to identity for `row`.
        let line = r#"{"action":"U","schema":"public","table":"t","columns":[],"identity":[{"name":"id","value":5}]}"#;
        let parsed = parse_wal2json_row(line).unwrap();
        assert!(parsed.columns.is_empty());
        assert_eq!(build_envelope(&parsed, "0/1").row.get("id").unwrap(), 5);
        // Insert with empty columns AND no identity yields an empty row object
        // (not null) and no previousRow.
        let line2 = r#"{"action":"I","schema":"public","table":"t","columns":[]}"#;
        let parsed2 = parse_wal2json_row(line2).unwrap();
        let env2 = build_envelope(&parsed2, "0/1");
        assert_eq!(env2.row, serde_json::json!({}));
        assert!(env2.previous_row.is_none());
    }

    #[test]
    fn envelope_json_roundtrips_camelcase_and_emits_null_fields() {
        // Encode -> decode round-trip over the exact wire bytes the pump ships.
        let line = r#"{"action":"U","schema":"public","table":"t","columns":[{"name":"id","value":1},{"name":"v","value":"x"}],"identity":[{"name":"id","value":1}]}"#;
        let parsed = parse_wal2json_row(line).unwrap();
        let env = build_envelope(&parsed, "0/DEADBEEF");
        let bytes = serde_json::to_vec(&env).expect("encode");
        let back: Value = serde_json::from_slice(&bytes).expect("decode");
        assert_eq!(back.get("schemaVersion").unwrap(), "cdc.row.v1");
        assert_eq!(back.get("op").unwrap(), "update");
        assert_eq!(back.get("lsn").unwrap(), "0/DEADBEEF");
        assert_eq!(back.get("primaryKey").unwrap(), &serde_json::json!(["id"]));
        assert!(back.get("tsMs").unwrap().is_u64());
        assert_eq!(back.get("row").unwrap().get("v").unwrap(), "x");
        assert_eq!(back.get("previousRow").unwrap().get("id").unwrap(), 1);
        // Absent source timestamp is still emitted as a null key on the wire.
        assert_eq!(back.get("sourceTimestamp").unwrap(), &Value::Null);

        // Insert: xid / sourceTimestamp / previousRow are all absent in the
        // source and are serialized as explicit JSON null (no skip_serializing).
        let ins =
            parse_wal2json_row(r#"{"action":"I","table":"t","columns":[{"name":"id","value":1}]}"#)
                .unwrap();
        let ienv = build_envelope(&ins, "0/1");
        let ib: Value = serde_json::from_slice(&serde_json::to_vec(&ienv).unwrap()).unwrap();
        assert_eq!(ib.get("xid").unwrap(), &Value::Null);
        assert_eq!(ib.get("sourceTimestamp").unwrap(), &Value::Null);
        assert_eq!(ib.get("previousRow").unwrap(), &Value::Null);
        assert!(ib.as_object().unwrap().contains_key("previousRow"));
    }

    // ── Integrity: corruption handling of an incoming record ──────────────

    #[test]
    fn corruption_that_breaks_json_framing_is_rejected() {
        let valid =
            r#"{"action":"I","schema":"public","table":"t","columns":[{"name":"id","value":1}]}"#;
        assert!(parse_wal2json_row(valid).is_some());
        // Flip the opening brace -> invalid JSON -> rejected, no panic.
        let mut bytes = valid.as_bytes().to_vec();
        bytes[0] = b'x';
        let corrupted = String::from_utf8(bytes).unwrap();
        assert!(parse_wal2json_row(&corrupted).is_none());
        // Drop the closing braces (tail truncation) -> rejected, no panic.
        assert!(parse_wal2json_row(&valid[..valid.len() - 3]).is_none());
    }

    #[test]
    fn in_payload_corruption_is_silently_accepted() {
        // FINDING / documented behavior: the gateway has NO per-record checksum.
        // A single-byte flip that keeps the line valid JSON is NOT detected —
        // the corrupted value is parsed and would be published verbatim. This
        // test pins that current behavior; an integrity control added later
        // should intentionally break it.
        let original = r#"{"action":"I","schema":"public","table":"t","columns":[{"name":"amount","value":"100"}]}"#;
        assert_eq!(
            parse_wal2json_row(original)
                .unwrap()
                .columns
                .get("amount")
                .unwrap(),
            "100"
        );
        let corrupted = original.replace("\"100\"", "\"900\"");
        assert_eq!(
            parse_wal2json_row(&corrupted)
                .expect("still valid json, still parses")
                .columns
                .get("amount")
                .unwrap(),
            "900" // wrong value accepted with no error
        );
    }

    // ── Subject construction & consumer round-trip ────────────────────────

    #[test]
    fn subject_format_per_op_and_custom_prefix() {
        let ins = parse_wal2json_row(
            r#"{"action":"I","schema":"public","table":"app_config","columns":[{"name":"id","value":1}]}"#,
        )
        .unwrap();
        assert_eq!(ins.subject("cdc"), "cdc.public.app_config.insert");
        assert_eq!(ins.subject("myprefix"), "myprefix.public.app_config.insert");

        let upd = parse_wal2json_row(
            r#"{"action":"U","schema":"billing","table":"invoices","columns":[{"name":"id","value":1}],"identity":[{"name":"id","value":1}]}"#,
        )
        .unwrap();
        assert_eq!(upd.subject("cdc"), "cdc.billing.invoices.update");

        let del = parse_wal2json_row(
            r#"{"action":"D","schema":"public","table":"lambda_functions","identity":[{"name":"id","value":1}]}"#,
        )
        .unwrap();
        assert_eq!(del.subject("cdc"), "cdc.public.lambda_functions.delete");
    }

    #[test]
    fn subject_roundtrips_through_consumer_parser() {
        use dd_nats_subject_defs::parse_cdc_row_change_subject;
        let upd = parse_wal2json_row(
            r#"{"action":"U","schema":"public","table":"orders","columns":[{"name":"id","value":1}],"identity":[{"name":"id","value":1}]}"#,
        )
        .unwrap();
        let subject = upd.subject("cdc");
        let parts = parse_cdc_row_change_subject(&subject).expect("consumer can parse the subject");
        assert_eq!(parts.prefix, "cdc");
        assert_eq!(parts.schema, "public");
        assert_eq!(parts.table, "orders");
        assert_eq!(parts.op, "update");
    }

    // ── Executable HTTP/OpenAPI contract invariants ──────────────────────

    #[test]
    fn openapi_is_deterministic_complete_and_fail_closed() {
        let first = docs::canonical_json(&openapi_document()).expect("serialize first contract");
        let second = docs::canonical_json(&openapi_document()).expect("serialize second contract");
        assert_eq!(
            first, second,
            "OpenAPI generation must be byte deterministic"
        );

        let document: Value = serde_json::from_str(&first).expect("parse generated OpenAPI");
        assert_eq!(document["openapi"], "3.1.0");
        assert_eq!(document["info"]["title"], "wal-gateway-rs API");
        assert_eq!(document["x-dd-contract-scope"], "internal");
        assert_eq!(document["x-dd-source-of-truth"], "utoipa-axum");
        assert!(
            document["components"]["securitySchemes"]["runtime_config_server_auth"].is_object()
        );

        let paths = document["paths"].as_object().expect("paths object");
        let actual = paths.keys().map(String::as_str).collect::<BTreeSet<_>>();
        let expected = EXPECTED_OPENAPI_PATHS
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected, "router/spec path parity changed");

        let public = BTreeSet::from([
            "/",
            "/api/docs",
            "/api/docs.json",
            "/docs/api",
            "/openapi.json",
        ]);
        let mut operation_ids = BTreeSet::new();
        let mut operation_count = 0;
        for (path, item) in paths {
            let item = item.as_object().expect("path item object");
            for (method, operation) in item {
                if ![
                    "get", "post", "put", "patch", "delete", "head", "options", "trace",
                ]
                .contains(&method.as_str())
                {
                    continue;
                }
                operation_count += 1;
                let operation_id = operation["operationId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{method} {path} has no operationId"));
                assert!(
                    operation_ids.insert(operation_id.to_string()),
                    "duplicate operationId {operation_id}"
                );
                assert!(
                    operation["responses"]
                        .as_object()
                        .is_some_and(|responses| !responses.is_empty()),
                    "{method} {path} has no responses"
                );
                assert_eq!(
                    operation["x-dd-visibility"],
                    if public.contains(path.as_str()) {
                        "public"
                    } else {
                        "internal"
                    },
                    "{method} {path} has the wrong visibility"
                );
            }
        }
        assert_eq!(operation_count, EXPECTED_OPENAPI_PATHS.len());
        assert_eq!(operation_ids.len(), EXPECTED_OPENAPI_PATHS.len());

        let root_schema = &document["components"]["schemas"]["RootResponse"];
        let root_properties = root_schema["properties"]
            .as_object()
            .expect("RootResponse properties");
        assert_eq!(
            root_properties
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["ok", "schemaVersion", "service"])
        );
        let encoded_root = root_schema.to_string().to_ascii_lowercase();
        for forbidden in [
            "pod",
            "slot",
            "stream",
            "subjectprefix",
            "leader",
            "lastlsn",
            "database_url",
            "nats_url",
        ] {
            assert!(
                !encoded_root.contains(forbidden),
                "public RootResponse leaked {forbidden}"
            );
        }
    }

    #[tokio::test]
    async fn runtime_docs_serve_only_the_embedded_public_contract() {
        let openapi = openapi_document();
        let canonical = docs::canonical_json(&openapi).expect("canonical internal JSON");
        let public = docs::public_json();
        assert_ne!(
            public, canonical,
            "public and internal contracts must differ"
        );

        let public_document: Value = serde_json::from_str(public).expect("parse public OpenAPI");
        assert_eq!(public_document["x-dd-contract-scope"], "public");
        let public_paths = public_document["paths"]
            .as_object()
            .expect("public paths")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            public_paths,
            BTreeSet::from([
                "/",
                "/api/docs",
                "/api/docs.json",
                "/docs/api",
                "/openapi.json",
            ])
        );
        assert!(public_document["paths"]["/healthz"].is_null());
        assert!(public_document["paths"]["/internal/runtime-config"].is_null());

        let shared = Arc::new(ApiDocs::new(&openapi).expect("runtime docs"));
        assert_eq!(shared.internal_json.as_ref(), canonical.as_bytes());

        for response in [
            openapi_json(Extension(shared.clone())).await,
            api_docs_json(Extension(shared.clone())).await,
        ] {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                OPENAPI_CONTENT_TYPE
            );
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("read public OpenAPI body");
            assert_eq!(body.as_ref(), public.as_bytes());
        }

        for response in [
            api_docs_ui(Extension(shared.clone())).await,
            docs_api_ui(Extension(shared.clone())).await,
        ] {
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("read public Scalar body");
            let html = String::from_utf8(body.to_vec()).expect("UTF-8 Scalar HTML");
            assert!(html.to_ascii_lowercase().contains("scalar"));
            assert!(html.contains("wal-gateway-rs API (public)"));
        }
    }

    #[tokio::test]
    async fn public_root_response_excludes_operational_state() {
        let response = root().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read root response");
        let value: Value = serde_json::from_slice(&body).expect("parse root response");
        assert_eq!(value["ok"], true);
        assert_eq!(value["service"], SERVICE_NAME);
        assert_eq!(value["schemaVersion"], SCHEMA_VERSION);
        for forbidden in [
            "pod",
            "slot",
            "stream",
            "subjectPrefix",
            "leader",
            "lastLsn",
            "databaseUrl",
            "natsUrl",
        ] {
            assert!(value.get(forbidden).is_none(), "root leaked {forbidden}");
        }
    }

    // ── Misc invariants ───────────────────────────────────────────────────

    #[test]
    fn leader_lock_key_is_stable() {
        // Every replica must derive the SAME advisory-lock key or leader
        // election silently breaks (two writers on one slot). Pin both the
        // ASCII intent and the exact value so any drift is caught at test time.
        assert_eq!(LEADER_LOCK_KEY, i64::from_be_bytes(*b"WALGATEW"));
        assert_eq!(LEADER_LOCK_KEY, 6287390423708353879);
    }

    #[test]
    fn default_stream_name_tracks_subject_defs_source_of_truth() {
        assert_eq!(DEFAULT_STREAM_NAME, "CDC");
        assert_eq!(DEFAULT_STREAM_NAME, CDC_STREAM_NAME);
    }

    #[test]
    fn now_ms_returns_plausible_epoch_millis() {
        // A fixed lower bound (2020-01-01T00:00:00Z in ms) proves now_ms emits
        // epoch-milliseconds rather than 0 or seconds.
        assert!(now_ms() > 1_577_836_800_000, "now_ms suspiciously small");
    }
}
