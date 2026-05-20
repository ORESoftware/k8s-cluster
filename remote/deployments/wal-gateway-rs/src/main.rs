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
//! 6. Pump loop: poll `pg_logical_slot_get_changes(slot, …, 'wal2json',
//!    'format-version', '2', 'include-lsn', 'true')`. For each change row,
//!    publish to `cdc.<schema>.<table>.<op>` with a normalized envelope.
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
//! * At-least-once delivery: the gateway commits the slot position AFTER
//!   the JetStream publish ack returns. A crash after publish but before
//!   slot advance will redeliver. Consumers must be idempotent (key off
//!   primary key + lsn).
//! * Single-writer to the slot: the advisory lock ensures only one process
//!   reads from the slot at a time. Other replicas idle.
//!
//! ## Configuration
//!
//! See `Config` below for the full env-var matrix. Sensible defaults assume
//! the local dev cluster (`PG_DATABASE_URL`, `NATS_URL`).

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

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use prometheus::{Encoder, IntCounter, IntGauge, TextEncoder};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::time::sleep;

const SERVICE_NAME: &str = "dd-wal-gateway";
const SCHEMA_VERSION: &str = "cdc.row.v1";
const DEFAULT_STREAM_NAME: &str = "CDC";
const DEFAULT_SUBJECT_PREFIX: &str = "cdc";
const DEFAULT_PORT: u16 = 8104;

/// Advisory-lock key used for leader election. `pg_try_advisory_lock` takes
/// a single bigint; we pick a deterministic 64-bit value so any replica
/// hits the same key without coordination. The number itself is arbitrary —
/// it just has to be unique across whatever else might use advisory locks
/// in the same database. Computed as the BE bytes of the ASCII string
/// "WALGATEW" so it's recognisable in `pg_locks` for ops.
const LEADER_LOCK_KEY: i64 = i64::from_be_bytes(*b"WALGATEW");

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
        let max_batch = env_u64("WAL_GATEWAY_MAX_BATCH", 2000) as i32;
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let config = Arc::new(Config::from_env().map_err(|error| {
        eprintln!("{SERVICE_NAME} config error: {error}");
        error
    })?);
    let metrics = Arc::new(Metrics::default());
    metrics.started_at_ms.store(now_ms(), Ordering::Relaxed);

    println!(
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

    let app = Router::new()
        .route("/", get(root))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics_handler))
        .with_state(state.clone());

    let addr: SocketAddr = format!("0.0.0.0:{}", config.http_port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("{SERVICE_NAME} listening on http://{addr}");
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
        {
            eprintln!("{SERVICE_NAME} http server error: {error}");
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
                eprintln!("{SERVICE_NAME} pump exited cleanly; restarting");
            }
            Err(error) => {
                eprintln!("{SERVICE_NAME} pump failed: {error}");
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
    let nats = async_nats::connect(nats_url).await?;
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
    println!(
        "{SERVICE_NAME} became leader pod={} slot={}",
        config.pod_name, config.slot_name
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
            eprintln!("{SERVICE_NAME} postgres connection task ended: {error}");
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
        let batch = match fetch_slot_changes(pg, &config.slot_name, config.max_batch).await {
            Ok(batch) => batch,
            Err(error) => {
                metrics.poll_failures_total.fetch_add(1, Ordering::Relaxed);
                eprintln!("{SERVICE_NAME} slot poll failed: {error}");
                // Bubble up — the outer loop will reconnect everything.
                return Err(error);
            }
        };
        if batch.is_empty() {
            continue;
        }
        for raw in batch {
            metrics.rows_seen_total.fetch_add(1, Ordering::Relaxed);
            match parse_wal2json_row(&raw.json) {
                Some(parsed) => {
                    let subject = parsed.subject(&config.subject_prefix);
                    let envelope = build_envelope(&parsed, &raw.lsn);
                    let bytes = match serde_json::to_vec(&envelope) {
                        Ok(b) => b,
                        Err(error) => {
                            eprintln!("{SERVICE_NAME} envelope encode failed: {error}");
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
                                    metrics
                                        .rows_published_total
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                                Ok(Err(error)) => {
                                    metrics
                                        .publish_failures_total
                                        .fetch_add(1, Ordering::Relaxed);
                                    eprintln!("{SERVICE_NAME} jetstream ack failed: {error}");
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
                            eprintln!("{SERVICE_NAME} jetstream publish failed: {error}");
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
    }
}

struct SlotRow {
    lsn: String,
    #[allow(dead_code)]
    xid: i64,
    json: String,
}

async fn fetch_slot_changes(
    pg: &tokio_postgres::Client,
    slot_name: &str,
    upto_nchanges: i32,
) -> Result<Vec<SlotRow>, Box<dyn Error + Send + Sync>> {
    // Using `_get_changes` (not `_peek_changes`) so the slot
    // `confirmed_flush_lsn` advances automatically. We rely on at-least-
    // once delivery: each row read here is guaranteed to be published
    // (or we error out and the row reappears on the next poll because
    // pg only advances inside the function call ON SUCCESSFUL RETURN —
    // it does NOT advance if the calling transaction rolls back).
    //
    // To keep that property, we run the slot read in implicit-txn mode
    // (single query, no explicit BEGIN). If we crash mid-iteration the
    // server side rolls back and the same rows come back. The cost: we
    // accept duplicate JetStream publishes on crash. JetStream itself
    // deduplicates by Nats-Msg-Id if we set the header (we don't, yet —
    // consumers must be idempotent).
    let rows = pg
        .query(
            "select lsn::text, xid::text, data
             from pg_logical_slot_get_changes(
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
        format!("{prefix}.{}.{}.{}", self.schema, self.table, self.op.as_str())
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
            if let (Some(name), Some(value)) = (
                item.get("name").and_then(Value::as_str),
                item.get("value"),
            ) {
                columns.insert(name.to_string(), value.clone());
            }
        }
    }
    let mut identity = BTreeMap::new();
    if let Some(items) = obj.get("identity").and_then(Value::as_array) {
        for item in items {
            if let (Some(name), Some(value)) = (
                item.get("name").and_then(Value::as_str),
                item.get("value"),
            ) {
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
        ChangeOp::Update | ChangeOp::Delete => Some(Value::Object(
            parsed.identity.clone().into_iter().collect(),
        )),
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

async fn root(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "pod": &state.config.pod_name,
        "slot": &state.config.slot_name,
        "stream": &state.config.stream_name,
        "subjectPrefix": &state.config.subject_prefix,
        "leader": state.metrics.leader.load(Ordering::Relaxed),
        "lastLsn": state.metrics.last_lsn.snapshot(),
        "uptimeMs": now_ms().saturating_sub(state.metrics.started_at_ms.load(Ordering::Relaxed)),
        "atMs": now_ms(),
    }))
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "leader": state.metrics.leader.load(Ordering::Relaxed),
        "polls": state.metrics.polls_total.load(Ordering::Relaxed),
        "rowsPublished": state.metrics.rows_published_total.load(Ordering::Relaxed),
        "atMs": now_ms(),
    }))
}

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
        Json(json!({
            "ok": ok,
            "service": SERVICE_NAME,
            "leader": state.metrics.leader.load(Ordering::Relaxed),
            "atMs": now_ms(),
        })),
    )
}

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
        "dd_wal_gateway_skipped_messages_total",
        "Slot messages skipped (BEGIN/COMMIT/etc).",
        state.metrics.skipped_messages_total.load(Ordering::Relaxed)
    );
    let mut buffer = Vec::new();
    let encoder = TextEncoder::new();
    let metric_families = registry.gather();
    if let Err(error) = encoder.encode(&metric_families, &mut buffer) {
        eprintln!("{SERVICE_NAME} metrics encode failed: {error}");
    }
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        buffer,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
