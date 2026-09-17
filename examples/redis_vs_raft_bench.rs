//! Rough Redis / Broker / BrokerRaft lock benchmark.
//!
//! This intentionally ignores fencing tokens. It measures successful
//! acquire+release cycles for:
//! - Redis: SET key token NX PX ttl, then EVAL compare-and-del.
//! - Broker: POST /v1/lock, then POST /v1/unlock against one regular broker.
//! - BrokerRaft: POST /v1/lock, then POST /v1/unlock.
//!
//! The HTTP paths default to one short-lived connection per request, matching
//! the simple LB-facing API. Set BENCH_HTTP_KEEPALIVE=true to reuse one HTTP
//! socket per worker per endpoint during server-side CPU profiling.
//! Set BENCH_RAFT_METRICS=true to scrape BrokerRaft `/metrics` before and
//! after the Raft run and print selected replication/proxy/compaction deltas,
//! plus per-successful-cycle efficiency ratios. BENCH_RAFT_METRICS_ENDPOINTS
//! defaults to BENCH_RAFT, so leader-routed traffic can still guard every local
//! node when the endpoint list includes the full cluster. When metrics capture
//! is on, BENCH_RAFT_FAIL_ON_FULL_LOG=true fails the benchmark if steady-state
//! full-log guard counters move.
//! Optional BENCH_MIN_*_OPS_PER_SEC and BENCH_MAX_*_P99_MS thresholds fail the
//! benchmark when a target falls below the configured performance floor.
//! Optional BENCH_MIN_RAFT_CLIENT_BATCH_ENTRIES_PER_BATCH and
//! BENCH_MAX_RAFT_COMMIT_SLOT_WRITES_PER_CYCLE thresholds fail Raft metric runs
//! when batching efficiency regresses. Optional BENCH_MAX_RAFT_*_WRITE_US_PER_CYCLE
//! thresholds fail when durable storage time per successful cycle regresses.

use std::collections::BTreeMap;
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Barrier;
use tokio::time::timeout;

const REDIS_UNLOCK_LUA: &str =
    "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('del', KEYS[1]) else return 0 end";
const DEFAULT_IO_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_REDIS_ADDR: &str = "127.0.0.1:6379";
const DEFAULT_BROKER_ADDR: &str = "127.0.0.1:6971";
const DEFAULT_RAFT_ADDR: &str = "127.0.0.1:6972";
const DEFAULT_TARGET: &str = "redis-raft";
const MAX_HTTP_RESPONSE_HEADER_BYTES: usize = 64 * 1024;
const MAX_HTTP_RESPONSE_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Redis,
    Broker,
    Raft,
    BrokerRaft,
    RedisRaft,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RaftRoute {
    RoundRobin,
    Leader,
}

#[derive(Debug, Clone)]
struct Config {
    redis_addr: String,
    broker_addr: String,
    raft_addrs: Vec<String>,
    raft_metric_addrs: Vec<String>,
    raft_route: RaftRoute,
    workers: usize,
    keys: usize,
    duration: Duration,
    ttl_ms: u64,
    io_timeout: Duration,
    target: Target,
    auth_token: Option<String>,
    http_keep_alive: bool,
    capture_raft_metrics: bool,
    fail_on_raft_full_log: bool,
    fail_on_errors: bool,
    fail_on_zero_success: bool,
    perf_thresholds: PerfThresholds,
}

#[derive(Debug, Clone, Default)]
struct PerfThresholds {
    min_redis_ops_per_sec: Option<f64>,
    min_broker_ops_per_sec: Option<f64>,
    min_raft_ops_per_sec: Option<f64>,
    max_redis_p99_ms: Option<f64>,
    max_broker_p99_ms: Option<f64>,
    max_raft_p99_ms: Option<f64>,
    min_raft_client_batch_entries_per_batch: Option<f64>,
    max_raft_commit_slot_writes_per_cycle: Option<f64>,
    max_raft_log_append_write_us_per_cycle: Option<f64>,
    max_raft_commit_slot_write_us_per_cycle: Option<f64>,
}

#[derive(Debug, Default)]
struct WorkerStats {
    ok: u64,
    not_acquired: u64,
    errors: u64,
    latencies_us: Vec<u64>,
}

#[derive(Debug, Default)]
struct Summary {
    ok: u64,
    not_acquired: u64,
    errors: u64,
    latencies_us: Vec<u64>,
    metric_lines: Vec<String>,
    fatal_errors: Vec<String>,
}

impl Summary {
    fn add(&mut self, worker: WorkerStats) {
        self.ok += worker.ok;
        self.not_acquired += worker.not_acquired;
        self.errors += worker.errors;
        self.latencies_us.extend(worker.latencies_us);
    }

    fn throughput(&self, duration: Duration) -> f64 {
        if duration.is_zero() {
            return 0.0;
        }
        self.ok as f64 / duration.as_secs_f64()
    }

    fn p99_ms(&self) -> f64 {
        let mut latencies = self.latencies_us.clone();
        latencies.sort_unstable();
        percentile_ms(&latencies, 0.99)
    }
}

#[derive(Debug)]
enum RedisValue {
    Simple(String),
    Error(String),
    Integer(i64),
    Bulk(Option<Vec<u8>>),
    Array,
}

#[tokio::main]
async fn main() {
    if env::args()
        .skip(1)
        .any(|arg| arg == "--help" || arg == "-h")
    {
        print_usage();
        return;
    }

    let config = Config::from_env();
    println!(
        "workers={} keys={} duration_ms={} ttl_ms={} io_timeout_ms={} redis={} broker={} raft={} raft_route={}",
        config.workers,
        config.keys,
        config.duration.as_millis(),
        config.ttl_ms,
        config.io_timeout.as_millis(),
        config.redis_addr,
        config.broker_addr,
        config.raft_addrs.join(","),
        config.raft_route.label()
    );
    if config.http_keep_alive {
        println!("http_keep_alive=true");
    }
    if config.capture_raft_metrics {
        println!("raft_metrics={}", config.raft_metric_addrs.join(","));
    }

    let mut redis_summary = None;
    let mut broker_summary = None;
    let mut raft_summary = None;
    let mut fatal_errors = Vec::new();

    if matches!(config.target, Target::Redis | Target::RedisRaft) {
        let summary = run_redis(config.clone()).await;
        print_summary("redis", &summary, config.duration);
        fatal_errors.extend(config.perf_guard_failures("redis", &summary));
        redis_summary = Some(summary);
    }
    if matches!(
        config.target,
        Target::Broker | Target::BrokerRaft | Target::All
    ) {
        let summary = run_broker(config.clone()).await;
        print_summary("broker", &summary, config.duration);
        fatal_errors.extend(config.perf_guard_failures("broker", &summary));
        broker_summary = Some(summary);
    }
    if matches!(
        config.target,
        Target::Raft | Target::RedisRaft | Target::BrokerRaft | Target::All
    ) {
        let summary = run_raft(config.clone()).await;
        print_summary("raft", &summary, config.duration);
        print_metric_lines(&summary.metric_lines);
        fatal_errors.extend(summary.fatal_errors.iter().cloned());
        fatal_errors.extend(config.perf_guard_failures("raft", &summary));
        raft_summary = Some(summary);
    }
    if matches!(config.target, Target::All) {
        let summary = run_redis(config.clone()).await;
        print_summary("redis", &summary, config.duration);
        fatal_errors.extend(config.perf_guard_failures("redis", &summary));
        redis_summary = Some(summary);
    }

    if let (Some(broker), Some(raft)) = (&broker_summary, &raft_summary) {
        print_ratio("broker", broker, "raft", raft);
    }
    if let (Some(redis), Some(raft)) = (&redis_summary, &raft_summary) {
        print_ratio("redis", redis, "raft", raft);
    }
    if !fatal_errors.is_empty() {
        for error in &fatal_errors {
            eprintln!("{error}");
        }
        std::process::exit(2);
    }
}

impl Config {
    fn from_env() -> Self {
        let target_value = env_string("BENCH_TARGET").unwrap_or_else(|| DEFAULT_TARGET.into());
        let target = match parse_target(&target_value) {
            Ok(target) => target,
            Err(other) => panic!(
                "BENCH_TARGET must be redis, broker, raft, broker-raft, redis-raft, or all; got {other:?}"
            ),
        };
        let raft_route_value =
            env_string("BENCH_RAFT_ROUTE").unwrap_or_else(|| "round-robin".into());
        let raft_route = match parse_raft_route(&raft_route_value) {
            Ok(route) => route,
            Err(other) => panic!("BENCH_RAFT_ROUTE must be round-robin or leader; got {other:?}"),
        };
        let workers = env_parse("BENCH_WORKERS", 8).max(1);
        let raft_addrs = parse_endpoint_list(
            &env_string("BENCH_RAFT").unwrap_or_else(|| DEFAULT_RAFT_ADDR.into()),
        );
        let raft_metric_addrs = env_string("BENCH_RAFT_METRICS_ENDPOINTS")
            .map(|value| parse_endpoint_list_with_default(&value, &raft_addrs))
            .unwrap_or_else(|| raft_addrs.clone());
        Self {
            redis_addr: env_string("BENCH_REDIS").unwrap_or_else(|| DEFAULT_REDIS_ADDR.into()),
            broker_addr: env_string("BENCH_BROKER").unwrap_or_else(|| DEFAULT_BROKER_ADDR.into()),
            raft_addrs,
            raft_metric_addrs,
            raft_route,
            workers,
            keys: env_parse("BENCH_KEYS", workers * 16).max(1),
            duration: Duration::from_millis(env_parse("BENCH_DURATION_MS", 10_000)),
            ttl_ms: env_parse("BENCH_TTL_MS", 5_000),
            io_timeout: Duration::from_millis(env_parse(
                "BENCH_IO_TIMEOUT_MS",
                DEFAULT_IO_TIMEOUT_MS,
            )),
            target,
            auth_token: env_string("BENCH_HTTP_AUTH_TOKEN")
                .or_else(|| env_string("BENCH_RAFT_AUTH_TOKEN"))
                .or_else(|| env_string("LMX_LIVE_RAFT_AUTH_TOKEN")),
            http_keep_alive: env_bool("BENCH_HTTP_KEEPALIVE", false),
            capture_raft_metrics: env_bool("BENCH_RAFT_METRICS", false),
            fail_on_raft_full_log: env_bool("BENCH_RAFT_FAIL_ON_FULL_LOG", true),
            fail_on_errors: env_bool("BENCH_FAIL_ON_ERRORS", true),
            fail_on_zero_success: env_bool("BENCH_FAIL_ON_ZERO_SUCCESS", true),
            perf_thresholds: PerfThresholds::from_env(),
        }
    }

    fn perf_guard_failures(&self, target: &str, summary: &Summary) -> Vec<String> {
        let mut failures = Vec::new();
        if self.fail_on_errors && summary.errors > 0 {
            failures.push(format!(
                "[perf-guard] {target} reported {} worker/request error(s); set BENCH_FAIL_ON_ERRORS=false to report without failing",
                summary.errors
            ));
        }
        if self.fail_on_zero_success && summary.ok == 0 {
            failures.push(format!(
                "[perf-guard] {target} completed 0 successful acquire/release cycles; set BENCH_FAIL_ON_ZERO_SUCCESS=false to report without failing"
            ));
        }
        if let Some(min_ops) = self.perf_thresholds.min_ops_per_sec(target) {
            let actual = summary.throughput(self.duration);
            if actual < min_ops {
                failures.push(format!(
                    "[perf-guard] {target} throughput {:.3} ops/s below BENCH_MIN_{}_OPS_PER_SEC {:.3}",
                    actual,
                    target_env_name(target),
                    min_ops
                ));
            }
        }
        if let Some(max_p99_ms) = self.perf_thresholds.max_p99_ms(target) {
            let actual = summary.p99_ms();
            if actual > max_p99_ms {
                failures.push(format!(
                    "[perf-guard] {target} p99 {:.3} ms above BENCH_MAX_{}_P99_MS {:.3}",
                    actual,
                    target_env_name(target),
                    max_p99_ms
                ));
            }
        }
        failures
    }
}

impl PerfThresholds {
    fn from_env() -> Self {
        Self {
            min_redis_ops_per_sec: env_parse_optional_non_negative_f64(
                "BENCH_MIN_REDIS_OPS_PER_SEC",
            ),
            min_broker_ops_per_sec: env_parse_optional_non_negative_f64(
                "BENCH_MIN_BROKER_OPS_PER_SEC",
            ),
            min_raft_ops_per_sec: env_parse_optional_non_negative_f64("BENCH_MIN_RAFT_OPS_PER_SEC"),
            max_redis_p99_ms: env_parse_optional_non_negative_f64("BENCH_MAX_REDIS_P99_MS"),
            max_broker_p99_ms: env_parse_optional_non_negative_f64("BENCH_MAX_BROKER_P99_MS"),
            max_raft_p99_ms: env_parse_optional_non_negative_f64("BENCH_MAX_RAFT_P99_MS"),
            min_raft_client_batch_entries_per_batch: env_parse_optional_non_negative_f64(
                "BENCH_MIN_RAFT_CLIENT_BATCH_ENTRIES_PER_BATCH",
            ),
            max_raft_commit_slot_writes_per_cycle: env_parse_optional_non_negative_f64(
                "BENCH_MAX_RAFT_COMMIT_SLOT_WRITES_PER_CYCLE",
            ),
            max_raft_log_append_write_us_per_cycle: env_parse_optional_non_negative_f64(
                "BENCH_MAX_RAFT_LOG_APPEND_WRITE_US_PER_CYCLE",
            ),
            max_raft_commit_slot_write_us_per_cycle: env_parse_optional_non_negative_f64(
                "BENCH_MAX_RAFT_COMMIT_SLOT_WRITE_US_PER_CYCLE",
            ),
        }
    }

    fn min_ops_per_sec(&self, target: &str) -> Option<f64> {
        match target {
            "redis" => self.min_redis_ops_per_sec,
            "broker" => self.min_broker_ops_per_sec,
            "raft" => self.min_raft_ops_per_sec,
            _ => None,
        }
    }

    fn max_p99_ms(&self, target: &str) -> Option<f64> {
        match target {
            "redis" => self.max_redis_p99_ms,
            "broker" => self.max_broker_p99_ms,
            "raft" => self.max_raft_p99_ms,
            _ => None,
        }
    }

    fn raft_metric_guard_failures(
        &self,
        before: &MetricSnapshot,
        after: &MetricSnapshot,
        successful_cycles: u64,
    ) -> Vec<String> {
        let mut failures = Vec::new();
        if let Some(min_entries_per_batch) = self.min_raft_client_batch_entries_per_batch {
            let client_batches = metric_delta(
                before,
                after,
                "dd_rust_network_mutex_raft_client_batches_total",
            );
            let client_entries = metric_delta(
                before,
                after,
                "dd_rust_network_mutex_raft_client_batch_entries_total",
            );
            if client_batches <= 0.0 {
                failures.push(format!(
                    "[raft-metrics-guard] client batch efficiency unavailable because client_batches={} while BENCH_MIN_RAFT_CLIENT_BATCH_ENTRIES_PER_BATCH={:.3}",
                    format_metric_rate(client_batches),
                    min_entries_per_batch
                ));
            } else {
                let actual = client_entries / client_batches;
                if actual < min_entries_per_batch {
                    failures.push(format!(
                        "[raft-metrics-guard] client batch entries per batch {:.3} below BENCH_MIN_RAFT_CLIENT_BATCH_ENTRIES_PER_BATCH {:.3}",
                        actual,
                        min_entries_per_batch
                    ));
                }
            }
        }
        if let Some(max_commit_writes_per_cycle) = self.max_raft_commit_slot_writes_per_cycle {
            if successful_cycles == 0 {
                failures.push(format!(
                    "[raft-metrics-guard] commit-slot writes per cycle unavailable because raft completed 0 successful cycles while BENCH_MAX_RAFT_COMMIT_SLOT_WRITES_PER_CYCLE={:.3}",
                    max_commit_writes_per_cycle
                ));
            } else {
                let commit_slot_writes = metric_delta(
                    before,
                    after,
                    "dd_rust_network_mutex_raft_hard_state_commit_slot_writes_total",
                );
                let actual = commit_slot_writes / successful_cycles as f64;
                if actual > max_commit_writes_per_cycle {
                    failures.push(format!(
                        "[raft-metrics-guard] commit-slot writes per cycle {:.3} above BENCH_MAX_RAFT_COMMIT_SLOT_WRITES_PER_CYCLE {:.3}",
                        actual,
                        max_commit_writes_per_cycle
                    ));
                }
            }
        }
        if let Some(max_us_per_cycle) = self.max_raft_log_append_write_us_per_cycle {
            if let Some(failure) = raft_metric_per_cycle_ceiling_failure(
                before,
                after,
                successful_cycles,
                "dd_rust_network_mutex_raft_log_append_write_us_total",
                "log append write us",
                "BENCH_MAX_RAFT_LOG_APPEND_WRITE_US_PER_CYCLE",
                max_us_per_cycle,
            ) {
                failures.push(failure);
            }
        }
        if let Some(max_us_per_cycle) = self.max_raft_commit_slot_write_us_per_cycle {
            if let Some(failure) = raft_metric_per_cycle_ceiling_failure(
                before,
                after,
                successful_cycles,
                "dd_rust_network_mutex_raft_hard_state_commit_slot_write_us_total",
                "commit-slot write us",
                "BENCH_MAX_RAFT_COMMIT_SLOT_WRITE_US_PER_CYCLE",
                max_us_per_cycle,
            ) {
                failures.push(failure);
            }
        }
        failures
    }
}

fn parse_target(value: &str) -> Result<Target, String> {
    match value.trim() {
        "redis" => Ok(Target::Redis),
        "broker" => Ok(Target::Broker),
        "raft" => Ok(Target::Raft),
        "broker-raft" | "brokervsraft" | "broker_vs_raft" => Ok(Target::BrokerRaft),
        "redis-raft" | "redis_vs_raft" | "both" => Ok(Target::RedisRaft),
        "all" => Ok(Target::All),
        other => Err(other.to_string()),
    }
}

fn parse_raft_route(value: &str) -> Result<RaftRoute, String> {
    match value.trim() {
        "round-robin" | "round_robin" | "rr" | "lb" => Ok(RaftRoute::RoundRobin),
        "leader" | "leader-preferred" | "leader_preferred" => Ok(RaftRoute::Leader),
        other => Err(other.to_string()),
    }
}

impl RaftRoute {
    fn label(self) -> &'static str {
        match self {
            Self::RoundRobin => "round-robin",
            Self::Leader => "leader",
        }
    }
}

async fn run_redis(config: Config) -> Summary {
    let barrier = Arc::new(Barrier::new(config.workers));
    let deadline = Instant::now() + config.duration;
    let mut handles = Vec::new();
    for worker_id in 0..config.workers {
        let cfg = config.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            redis_worker(cfg, worker_id, deadline).await
        }));
    }
    collect(handles).await
}

async fn redis_worker(config: Config, worker_id: usize, deadline: Instant) -> WorkerStats {
    let mut stats = WorkerStats::default();
    let mut conn = match RedisConn::connect(&config.redis_addr, config.io_timeout).await {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("redis worker {worker_id} connect error: {err}");
            stats.errors += 1;
            return stats;
        }
    };
    let mut seq = 0u64;
    let mut rng = worker_id as u64 + 1;
    while Instant::now() < deadline {
        seq += 1;
        let key = bench_key("redis", next_key(&mut rng, config.keys));
        let token = format!("{worker_id}-{seq}-{rng}");
        let start = Instant::now();
        match redis_lock_cycle(&mut conn, &key, &token, config.ttl_ms, config.io_timeout).await {
            Ok(true) => {
                stats.ok += 1;
                stats.latencies_us.push(start.elapsed().as_micros() as u64);
            }
            Ok(false) => stats.not_acquired += 1,
            Err(err) => {
                stats.errors += 1;
                eprintln!("redis worker {worker_id} error: {err}");
                match RedisConn::connect(&config.redis_addr, config.io_timeout).await {
                    Ok(new_conn) => conn = new_conn,
                    Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                }
            }
        }
    }
    stats
}

async fn redis_lock_cycle(
    conn: &mut RedisConn,
    key: &str,
    token: &str,
    ttl_ms: u64,
    io_timeout: Duration,
) -> Result<bool, String> {
    let set = conn
        .command(
            &["SET", key, token, "NX", "PX", &ttl_ms.to_string()],
            io_timeout,
        )
        .await?;
    match set {
        RedisValue::Simple(s) if s == "OK" => {}
        RedisValue::Bulk(None) => return Ok(false),
        RedisValue::Error(err) => return Err(err),
        other => return Err(format!("unexpected SET response: {other:?}")),
    }
    let unlock = conn
        .command(&["EVAL", REDIS_UNLOCK_LUA, "1", key, token], io_timeout)
        .await?;
    match unlock {
        RedisValue::Integer(1) => Ok(true),
        RedisValue::Integer(0) => Ok(false),
        RedisValue::Error(err) => Err(err),
        other => Err(format!("unexpected unlock response: {other:?}")),
    }
}

async fn run_broker(config: Config) -> Summary {
    let endpoint = config.broker_addr.clone();
    run_http_target(config, "broker", vec![endpoint]).await
}

async fn run_raft(config: Config) -> Summary {
    let endpoints = raft_benchmark_endpoints(&config).await;
    let before = if config.capture_raft_metrics {
        Some(capture_raft_metric_snapshot(&config, "before").await)
    } else {
        None
    };
    let mut summary = run_http_target(config.clone(), "raft", endpoints).await;
    if let Some(before) = before {
        let after = capture_raft_metric_snapshot(&config, "after").await;
        summary.metric_lines = raft_metric_delta_lines(&before, &after);
        summary
            .metric_lines
            .extend(raft_metric_per_cycle_lines(&before, &after, summary.ok));
        let failures = config
            .perf_thresholds
            .raft_metric_guard_failures(&before, &after, summary.ok);
        for failure in failures {
            summary.metric_lines.push(failure.clone());
            summary.fatal_errors.push(failure);
        }
        if config.fail_on_raft_full_log {
            let failures = raft_full_log_guard_failures(&before, &after);
            for failure in failures {
                let message = format!(
                    "[raft-metrics-guard] {failure}; set BENCH_RAFT_FAIL_ON_FULL_LOG=false to report without failing"
                );
                summary.metric_lines.push(message.clone());
                summary.fatal_errors.push(message);
            }
        }
    }
    summary
}

async fn raft_benchmark_endpoints(config: &Config) -> Vec<String> {
    match config.raft_route {
        RaftRoute::RoundRobin => config.raft_addrs.clone(),
        RaftRoute::Leader => match find_ready_raft_leader(config).await {
            Some(endpoint) => {
                println!("raft leader route selected {endpoint}");
                vec![endpoint]
            }
            None => {
                eprintln!(
                    "BENCH_RAFT_ROUTE=leader could not find a /raft/leaderz endpoint; falling back to round-robin configured endpoints"
                );
                config.raft_addrs.clone()
            }
        },
    }
}

async fn find_ready_raft_leader(config: &Config) -> Option<String> {
    for endpoint in &config.raft_addrs {
        match http_status(
            endpoint,
            "GET",
            "/raft/leaderz",
            config.auth_token.as_deref(),
            config.io_timeout,
        )
        .await
        {
            Ok(200) => return Some(endpoint.clone()),
            Ok(_) => continue,
            Err(err) => {
                eprintln!("leader probe {endpoint}/raft/leaderz failed: {err}");
            }
        }
    }
    None
}

async fn run_http_target(config: Config, name: &'static str, endpoints: Vec<String>) -> Summary {
    let barrier = Arc::new(Barrier::new(config.workers));
    let deadline = Instant::now() + config.duration;
    let endpoints = Arc::new(endpoints);
    let mut handles = Vec::new();
    for worker_id in 0..config.workers {
        let cfg = config.clone();
        let barrier = barrier.clone();
        let endpoints = endpoints.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            http_worker(cfg, name, endpoints, worker_id, deadline).await
        }));
    }
    collect(handles).await
}

async fn http_worker(
    config: Config,
    name: &str,
    endpoints: Arc<Vec<String>>,
    worker_id: usize,
    deadline: Instant,
) -> WorkerStats {
    let mut stats = WorkerStats::default();
    let mut seq = 0u64;
    let mut rng = worker_id as u64 + 17;
    let mut client = HttpWorkerClient::new(
        config.http_keep_alive,
        config.auth_token.clone(),
        config.io_timeout,
    );
    while Instant::now() < deadline {
        seq += 1;
        let key = bench_key(name, next_key(&mut rng, config.keys));
        let (acquire_endpoint, release_endpoint) = endpoints_for_cycle(&endpoints, worker_id, seq);
        let start = Instant::now();
        match http_lock_cycle(
            &mut client,
            &config,
            acquire_endpoint,
            release_endpoint,
            &key,
        )
        .await
        {
            Ok(true) => {
                stats.ok += 1;
                stats.latencies_us.push(start.elapsed().as_micros() as u64);
            }
            Ok(false) => stats.not_acquired += 1,
            Err(err) => {
                stats.errors += 1;
                eprintln!("{name} worker {worker_id} op {seq} error: {err}");
            }
        }
    }
    stats
}

struct HttpWorkerClient {
    keep_alive: bool,
    auth_token: Option<String>,
    io_timeout: Duration,
    connections: BTreeMap<String, HttpConn>,
}

impl HttpWorkerClient {
    fn new(keep_alive: bool, auth_token: Option<String>, io_timeout: Duration) -> Self {
        Self {
            keep_alive,
            auth_token,
            io_timeout,
            connections: BTreeMap::new(),
        }
    }

    async fn json(
        &mut self,
        endpoint: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value), String> {
        let (status, text) = if self.keep_alive {
            self.request_keep_alive(endpoint, method, path, body)
                .await?
        } else {
            timeout(
                self.io_timeout,
                http_request(endpoint, method, path, body, self.auth_token.as_deref()),
            )
            .await
            .map_err(|_| {
                format!(
                    "HTTP {method} {path} to {endpoint} timed out after {:?}",
                    self.io_timeout
                )
            })??
        };
        let parsed = serde_json::from_str(&text).map_err(|err| {
            format!("failed to parse HTTP JSON status={status}: {err}; body={text:?}")
        })?;
        if status / 100 == 2 {
            Ok((status, parsed))
        } else {
            Err(format!("HTTP {status}: {parsed:?}"))
        }
    }

    async fn request_keep_alive(
        &mut self,
        endpoint: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, String), String> {
        let connection = match self.connections.get_mut(endpoint) {
            Some(connection) => connection,
            None => {
                let connection = HttpConn::connect(endpoint, self.io_timeout).await?;
                self.connections.insert(endpoint.to_string(), connection);
                self.connections
                    .get_mut(endpoint)
                    .expect("inserted HTTP connection")
            }
        };
        match connection
            .request(
                method,
                path,
                body,
                self.auth_token.as_deref(),
                self.io_timeout,
            )
            .await
        {
            Ok(response) => Ok(response),
            Err(err) => {
                self.connections.remove(endpoint);
                Err(err)
            }
        }
    }
}

fn parse_endpoint_list(value: &str) -> Vec<String> {
    parse_endpoint_list_with_default(value, &[DEFAULT_RAFT_ADDR.to_string()])
}

fn parse_endpoint_list_with_default(value: &str, default: &[String]) -> Vec<String> {
    let endpoints = value
        .split(',')
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if endpoints.is_empty() {
        default.to_vec()
    } else {
        endpoints
    }
}

fn endpoint_for(endpoints: &[String], worker_id: usize, seq: u64) -> &str {
    let index = worker_id.wrapping_add(seq as usize).wrapping_sub(1) % endpoints.len().max(1);
    endpoints
        .get(index)
        .map(String::as_str)
        .unwrap_or(DEFAULT_RAFT_ADDR)
}

fn endpoints_for_cycle(endpoints: &[String], worker_id: usize, cycle_seq: u64) -> (&str, &str) {
    (
        endpoint_for(
            endpoints,
            worker_id,
            cycle_seq.saturating_mul(2).saturating_sub(1),
        ),
        endpoint_for(endpoints, worker_id, cycle_seq.saturating_mul(2)),
    )
}

async fn http_lock_cycle(
    client: &mut HttpWorkerClient,
    config: &Config,
    acquire_endpoint: &str,
    release_endpoint: &str,
    key: &str,
) -> Result<bool, String> {
    let (_, acquire) = client
        .json(
            acquire_endpoint,
            "POST",
            "/v1/lock",
            Some(json!({"key": key, "ttlMs": config.ttl_ms})),
        )
        .await?;
    if acquire["acquired"] != true {
        return Ok(false);
    }
    let lock_uuid = acquire["lockUuid"]
        .as_str()
        .ok_or_else(|| format!("missing lockUuid in acquire response: {acquire:?}"))?;
    let (_, release) = client
        .json(
            release_endpoint,
            "POST",
            "/v1/unlock",
            Some(json!({"key": key, "lockUuid": lock_uuid})),
        )
        .await?;
    if release["unlocked"] == true {
        Ok(true)
    } else {
        Err(format!("unlock failed: {release:?}"))
    }
}

async fn collect(handles: Vec<tokio::task::JoinHandle<WorkerStats>>) -> Summary {
    let mut summary = Summary::default();
    for handle in handles {
        match handle.await {
            Ok(stats) => summary.add(stats),
            Err(err) => {
                eprintln!("worker join error: {err}");
                summary.errors += 1;
            }
        }
    }
    summary
}

fn print_summary(name: &str, summary: &Summary, duration: Duration) {
    let mut latencies = summary.latencies_us.clone();
    latencies.sort_unstable();
    let throughput = summary.throughput(duration);
    let avg = if latencies.is_empty() {
        0.0
    } else {
        latencies.iter().sum::<u64>() as f64 / latencies.len() as f64 / 1000.0
    };
    println!(
        "{name:5} total={:>8} ops/s={:>10.0} avg_ms={:>8.3} p50_ms={:>8.3} p95_ms={:>8.3} p99_ms={:>8.3} max_ms={:>8.3} not_acquired={} errors={}",
        summary.ok,
        throughput,
        avg,
        percentile_ms(&latencies, 0.50),
        percentile_ms(&latencies, 0.95),
        percentile_ms(&latencies, 0.99),
        latencies.last().copied().unwrap_or(0) as f64 / 1000.0,
        summary.not_acquired,
        summary.errors,
    );
}

fn print_ratio(a_name: &str, a: &Summary, b_name: &str, b: &Summary) {
    if b.ok == 0 {
        println!("[ratio] {a_name}/{b_name}: unavailable because {b_name} completed 0 ops");
        return;
    }
    println!(
        "[ratio] {a_name}/{b_name} throughput = {:.2}x ({} / {} successful cycles)",
        a.ok as f64 / b.ok as f64,
        a.ok,
        b.ok
    );
}

fn print_metric_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

#[derive(Debug, Default)]
struct MetricSnapshot {
    successful_endpoints: usize,
    values: BTreeMap<String, f64>,
    errors: Vec<String>,
}

const RAFT_BENCH_METRICS: &[(&str, &str)] = &[
    (
        "append_rpc",
        "dd_rust_network_mutex_raft_append_entries_requests_total",
    ),
    (
        "append_batches",
        "dd_rust_network_mutex_raft_append_entries_batches_total",
    ),
    (
        "append_entries",
        "dd_rust_network_mutex_raft_append_entries_sent_total",
    ),
    (
        "append_log_bytes",
        "dd_rust_network_mutex_raft_append_entries_log_bytes_total",
    ),
    (
        "append_success",
        "dd_rust_network_mutex_raft_append_entries_successes_total",
    ),
    (
        "append_conflicts",
        "dd_rust_network_mutex_raft_append_entries_conflicts_total",
    ),
    (
        "append_rpc_errors",
        "dd_rust_network_mutex_raft_append_entries_rpc_errors_total",
    ),
    (
        "append_frame_mismatches",
        "dd_rust_network_mutex_raft_append_entries_frame_size_mismatches_total",
    ),
    (
        "admission_probes",
        "dd_rust_network_mutex_raft_client_admission_quorum_probes_total",
    ),
    (
        "admission_probe_success",
        "dd_rust_network_mutex_raft_client_admission_quorum_probe_successes_total",
    ),
    (
        "admission_probe_fail",
        "dd_rust_network_mutex_raft_client_admission_quorum_probe_failures_total",
    ),
    (
        "admission_probe_acks",
        "dd_rust_network_mutex_raft_client_admission_quorum_probe_acks_total",
    ),
    (
        "admission_probe_us",
        "dd_rust_network_mutex_raft_client_admission_quorum_probe_us_total",
    ),
    (
        "client_batches",
        "dd_rust_network_mutex_raft_client_batches_total",
    ),
    (
        "client_batch_entries",
        "dd_rust_network_mutex_raft_client_batch_entries_total",
    ),
    (
        "client_pipeline_batches",
        "dd_rust_network_mutex_raft_client_batch_pipeline_batches_total",
    ),
    (
        "client_queue_wait_us",
        "dd_rust_network_mutex_raft_client_batch_queue_wait_us_total",
    ),
    (
        "client_refill_rounds",
        "dd_rust_network_mutex_raft_client_batch_refill_rounds_total",
    ),
    (
        "client_refilled_entries",
        "dd_rust_network_mutex_raft_client_batch_refilled_entries_total",
    ),
    (
        "client_commit_waits",
        "dd_rust_network_mutex_raft_client_batch_commit_lock_waits_total",
    ),
    (
        "client_commit_wait_us",
        "dd_rust_network_mutex_raft_client_batch_commit_lock_wait_us_total",
    ),
    (
        "client_cancelled",
        "dd_rust_network_mutex_raft_client_batch_cancelled_requests_total",
    ),
    (
        "client_batch_errors",
        "dd_rust_network_mutex_raft_client_batch_errors_total",
    ),
    (
        "follower_conflicts",
        "dd_rust_network_mutex_raft_follower_append_conflicts_total",
    ),
    (
        "follower_rewrites",
        "dd_rust_network_mutex_raft_follower_append_rewrites_total",
    ),
    (
        "follower_appended",
        "dd_rust_network_mutex_raft_follower_append_appended_entries_total",
    ),
    (
        "follower_rewritten",
        "dd_rust_network_mutex_raft_follower_append_rewritten_entries_total",
    ),
    (
        "follower_truncated",
        "dd_rust_network_mutex_raft_follower_append_truncated_entries_total",
    ),
    (
        "follower_sender_rejects",
        "dd_rust_network_mutex_raft_follower_append_sender_rejections_total",
    ),
    (
        "snapshot_chunks",
        "dd_rust_network_mutex_raft_install_snapshot_chunks_total",
    ),
    (
        "snapshot_bytes",
        "dd_rust_network_mutex_raft_install_snapshot_bytes_total",
    ),
    (
        "snapshot_success",
        "dd_rust_network_mutex_raft_install_snapshot_successes_total",
    ),
    (
        "snapshot_frame_mismatches",
        "dd_rust_network_mutex_raft_install_snapshot_frame_size_mismatches_total",
    ),
    (
        "proxy_forwarded",
        "dd_rust_network_mutex_raft_proxy_requests_forwarded_total",
    ),
    (
        "proxy_errors",
        "dd_rust_network_mutex_raft_proxy_request_errors_total",
    ),
    (
        "quorum_waits",
        "dd_rust_network_mutex_raft_replication_quorum_waits_total",
    ),
    (
        "quorum_ms",
        "dd_rust_network_mutex_raft_replication_quorum_wait_ms_total",
    ),
    (
        "compactions",
        "dd_rust_network_mutex_raft_log_compactions_total",
    ),
    (
        "compaction_failures",
        "dd_rust_network_mutex_raft_log_compaction_failures_total",
    ),
    (
        "compaction_trim_failures",
        "dd_rust_network_mutex_raft_log_compaction_trim_failures_total",
    ),
    (
        "rewrite_tmp_cleanups",
        "dd_rust_network_mutex_raft_log_rewrite_temp_cleanups_total",
    ),
    (
        "log_rollbacks",
        "dd_rust_network_mutex_raft_log_write_rollbacks_total",
    ),
    (
        "log_append_opens",
        "dd_rust_network_mutex_raft_log_append_file_opens_total",
    ),
    (
        "log_append_write_us",
        "dd_rust_network_mutex_raft_log_append_write_us_total",
    ),
    (
        "log_append_cache_invalidations",
        "dd_rust_network_mutex_raft_log_append_file_cache_invalidations_total",
    ),
    (
        "full_log_reads",
        "dd_rust_network_mutex_raft_log_full_reads_total",
    ),
    (
        "full_log_read_failures",
        "dd_rust_network_mutex_raft_log_full_read_failures_total",
    ),
    (
        "full_log_read_bytes",
        "dd_rust_network_mutex_raft_log_full_read_bytes_total",
    ),
    (
        "full_log_read_entries",
        "dd_rust_network_mutex_raft_log_full_read_entries_total",
    ),
    (
        "full_log_rewrites",
        "dd_rust_network_mutex_raft_log_full_rewrites_total",
    ),
    (
        "full_log_rewrite_failures",
        "dd_rust_network_mutex_raft_log_full_rewrite_failures_total",
    ),
    (
        "full_log_rewrite_bytes",
        "dd_rust_network_mutex_raft_log_full_rewrite_bytes_total",
    ),
    (
        "full_log_rewrite_entries",
        "dd_rust_network_mutex_raft_log_full_rewrite_entries_total",
    ),
    (
        "commit_slot_writes",
        "dd_rust_network_mutex_raft_hard_state_commit_slot_writes_total",
    ),
    (
        "commit_slot_bytes",
        "dd_rust_network_mutex_raft_hard_state_commit_slot_write_bytes_total",
    ),
    (
        "commit_slot_write_us",
        "dd_rust_network_mutex_raft_hard_state_commit_slot_write_us_total",
    ),
    (
        "commit_slot_errors",
        "dd_rust_network_mutex_raft_hard_state_commit_slot_write_errors_total",
    ),
    (
        "commit_slot_opens",
        "dd_rust_network_mutex_raft_hard_state_commit_slot_file_opens_total",
    ),
    (
        "commit_slot_recoveries",
        "dd_rust_network_mutex_raft_hard_state_commit_slot_recoveries_total",
    ),
    (
        "commit_slot_invalid_recoveries",
        "dd_rust_network_mutex_raft_hard_state_commit_slot_invalid_recoveries_total",
    ),
    (
        "commit_slot_truncations",
        "dd_rust_network_mutex_raft_hard_state_commit_slot_truncations_total",
    ),
    ("log_bytes", "dd_rust_network_mutex_raft_log_bytes"),
    (
        "retained_entries",
        "dd_rust_network_mutex_raft_log_retained_entries",
    ),
    (
        "peer_max_lag",
        "dd_rust_network_mutex_raft_peer_max_lag_entries",
    ),
];

async fn capture_raft_metric_snapshot(config: &Config, phase: &str) -> MetricSnapshot {
    let mut snapshot = MetricSnapshot::default();
    for endpoint in &config.raft_metric_addrs {
        match timeout(
            config.io_timeout,
            http_request(
                endpoint,
                "GET",
                "/metrics",
                None,
                config.auth_token.as_deref(),
            ),
        )
        .await
        {
            Ok(Ok((status, body))) if status / 100 == 2 => {
                snapshot.successful_endpoints += 1;
                merge_metric_values(&mut snapshot.values, parse_prometheus_metrics(&body));
            }
            Ok(Ok((status, body))) => snapshot.errors.push(format!(
                "{phase} scrape {endpoint}/metrics returned HTTP {status}: {}",
                body.trim()
            )),
            Ok(Err(err)) => snapshot
                .errors
                .push(format!("{phase} scrape {endpoint}/metrics failed: {err}")),
            Err(_) => snapshot.errors.push(format!(
                "{phase} scrape {endpoint}/metrics timed out after {:?}",
                config.io_timeout
            )),
        }
    }
    snapshot
}

fn parse_prometheus_metrics(text: &str) -> BTreeMap<String, f64> {
    let mut values = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(raw_name) = parts.next() else {
            continue;
        };
        let Some(raw_value) = parts.next() else {
            continue;
        };
        let Some(name) = raw_name.split('{').next() else {
            continue;
        };
        let Ok(value) = raw_value.parse::<f64>() else {
            continue;
        };
        if value.is_finite() {
            *values.entry(name.to_string()).or_insert(0.0) += value;
        }
    }
    values
}

fn merge_metric_values(dst: &mut BTreeMap<String, f64>, src: BTreeMap<String, f64>) {
    for (name, value) in src {
        *dst.entry(name).or_insert(0.0) += value;
    }
}

fn raft_metric_delta_lines(before: &MetricSnapshot, after: &MetricSnapshot) -> Vec<String> {
    let mut lines = vec![format!(
        "[raft-metrics] sampled_endpoints_before={} sampled_endpoints_after={}",
        before.successful_endpoints, after.successful_endpoints
    )];
    for error in &before.errors {
        lines.push(format!("[raft-metrics] {error}"));
    }
    for error in &after.errors {
        lines.push(format!("[raft-metrics] {error}"));
    }

    let mut current = String::from("[raft-metrics]");
    for (label, metric_name) in RAFT_BENCH_METRICS {
        let before_value = before.values.get(*metric_name).copied().unwrap_or(0.0);
        let after_value = after.values.get(*metric_name).copied().unwrap_or(0.0);
        let part = format!(
            " {label}={}",
            format_metric_delta(after_value - before_value)
        );
        if current.len() + part.len() > 160 {
            lines.push(current);
            current = String::from("[raft-metrics]");
        }
        current.push_str(&part);
    }
    if current != "[raft-metrics]" {
        lines.push(current);
    }
    lines
}

const RAFT_BENCH_PER_CYCLE_METRICS: &[(&str, &str)] = &[
    (
        "append_rpc",
        "dd_rust_network_mutex_raft_append_entries_requests_total",
    ),
    (
        "append_entries",
        "dd_rust_network_mutex_raft_append_entries_sent_total",
    ),
    (
        "append_log_bytes",
        "dd_rust_network_mutex_raft_append_entries_log_bytes_total",
    ),
    (
        "admission_probe",
        "dd_rust_network_mutex_raft_client_admission_quorum_probes_total",
    ),
    (
        "admission_probe_us",
        "dd_rust_network_mutex_raft_client_admission_quorum_probe_us_total",
    ),
    (
        "client_batches",
        "dd_rust_network_mutex_raft_client_batches_total",
    ),
    (
        "client_batch_entries",
        "dd_rust_network_mutex_raft_client_batch_entries_total",
    ),
    (
        "client_pipeline_batches",
        "dd_rust_network_mutex_raft_client_batch_pipeline_batches_total",
    ),
    (
        "client_queue_wait_us",
        "dd_rust_network_mutex_raft_client_batch_queue_wait_us_total",
    ),
    (
        "client_refill_rounds",
        "dd_rust_network_mutex_raft_client_batch_refill_rounds_total",
    ),
    (
        "client_refilled_entries",
        "dd_rust_network_mutex_raft_client_batch_refilled_entries_total",
    ),
    (
        "client_commit_waits",
        "dd_rust_network_mutex_raft_client_batch_commit_lock_waits_total",
    ),
    (
        "client_commit_wait_us",
        "dd_rust_network_mutex_raft_client_batch_commit_lock_wait_us_total",
    ),
    (
        "follower_appended",
        "dd_rust_network_mutex_raft_follower_append_appended_entries_total",
    ),
    (
        "follower_rewrites",
        "dd_rust_network_mutex_raft_follower_append_rewrites_total",
    ),
    (
        "quorum_waits",
        "dd_rust_network_mutex_raft_replication_quorum_waits_total",
    ),
    (
        "proxy_forwarded",
        "dd_rust_network_mutex_raft_proxy_requests_forwarded_total",
    ),
    (
        "snapshot_chunks",
        "dd_rust_network_mutex_raft_install_snapshot_chunks_total",
    ),
    (
        "commit_slot_writes",
        "dd_rust_network_mutex_raft_hard_state_commit_slot_writes_total",
    ),
    (
        "commit_slot_bytes",
        "dd_rust_network_mutex_raft_hard_state_commit_slot_write_bytes_total",
    ),
    (
        "commit_slot_write_us",
        "dd_rust_network_mutex_raft_hard_state_commit_slot_write_us_total",
    ),
    (
        "log_append_opens",
        "dd_rust_network_mutex_raft_log_append_file_opens_total",
    ),
    (
        "log_append_write_us",
        "dd_rust_network_mutex_raft_log_append_write_us_total",
    ),
    (
        "full_log_reads",
        "dd_rust_network_mutex_raft_log_full_reads_total",
    ),
    (
        "full_log_read_failures",
        "dd_rust_network_mutex_raft_log_full_read_failures_total",
    ),
    (
        "full_log_read_bytes",
        "dd_rust_network_mutex_raft_log_full_read_bytes_total",
    ),
    (
        "full_log_read_entries",
        "dd_rust_network_mutex_raft_log_full_read_entries_total",
    ),
    (
        "full_log_rewrites",
        "dd_rust_network_mutex_raft_log_full_rewrites_total",
    ),
    (
        "full_log_rewrite_failures",
        "dd_rust_network_mutex_raft_log_full_rewrite_failures_total",
    ),
    (
        "full_log_rewrite_bytes",
        "dd_rust_network_mutex_raft_log_full_rewrite_bytes_total",
    ),
    (
        "full_log_rewrite_entries",
        "dd_rust_network_mutex_raft_log_full_rewrite_entries_total",
    ),
];

const RAFT_FULL_LOG_GUARD_METRICS: &[(&str, &str)] = &[
    (
        "full_log_reads",
        "dd_rust_network_mutex_raft_log_full_reads_total",
    ),
    (
        "full_log_read_failures",
        "dd_rust_network_mutex_raft_log_full_read_failures_total",
    ),
    (
        "full_log_read_bytes",
        "dd_rust_network_mutex_raft_log_full_read_bytes_total",
    ),
    (
        "full_log_read_entries",
        "dd_rust_network_mutex_raft_log_full_read_entries_total",
    ),
    (
        "full_log_rewrites",
        "dd_rust_network_mutex_raft_log_full_rewrites_total",
    ),
    (
        "full_log_rewrite_failures",
        "dd_rust_network_mutex_raft_log_full_rewrite_failures_total",
    ),
    (
        "full_log_rewrite_bytes",
        "dd_rust_network_mutex_raft_log_full_rewrite_bytes_total",
    ),
    (
        "full_log_rewrite_entries",
        "dd_rust_network_mutex_raft_log_full_rewrite_entries_total",
    ),
];

fn raft_metric_per_cycle_lines(
    before: &MetricSnapshot,
    after: &MetricSnapshot,
    successful_cycles: u64,
) -> Vec<String> {
    if successful_cycles == 0 {
        return vec![
            "[raft-metrics-per-cycle] unavailable because raft completed 0 successful cycles"
                .into(),
        ];
    }

    let mut lines = Vec::new();
    let mut current = format!("[raft-metrics-per-cycle] cycles={successful_cycles}");
    for (label, metric_name) in RAFT_BENCH_PER_CYCLE_METRICS {
        let delta = metric_delta(before, after, metric_name);
        let part = format!(
            " {label}={}",
            format_metric_rate(delta / successful_cycles as f64)
        );
        if current.len() + part.len() > 160 {
            lines.push(current);
            current = String::from("[raft-metrics-per-cycle]");
        }
        current.push_str(&part);
    }
    lines.push(current);
    lines
}

fn raft_full_log_guard_failures(before: &MetricSnapshot, after: &MetricSnapshot) -> Vec<String> {
    let mut deltas = Vec::new();
    for (label, metric_name) in RAFT_FULL_LOG_GUARD_METRICS {
        let delta = metric_delta(before, after, metric_name);
        if delta > 0.0 {
            deltas.push(format!("{label}={}", format_metric_delta(delta)));
        }
    }
    if deltas.is_empty() {
        Vec::new()
    } else {
        vec![format!(
            "full-log activity observed during raft benchmark window: {}",
            deltas.join(" ")
        )]
    }
}

fn metric_delta(before: &MetricSnapshot, after: &MetricSnapshot, metric_name: &str) -> f64 {
    after.values.get(metric_name).copied().unwrap_or(0.0)
        - before.values.get(metric_name).copied().unwrap_or(0.0)
}

fn raft_metric_per_cycle_ceiling_failure(
    before: &MetricSnapshot,
    after: &MetricSnapshot,
    successful_cycles: u64,
    metric_name: &str,
    label: &str,
    env_name: &str,
    max_value: f64,
) -> Option<String> {
    if successful_cycles == 0 {
        return Some(format!(
            "[raft-metrics-guard] {label} per cycle unavailable because raft completed 0 successful cycles while {env_name}={max_value:.3}"
        ));
    }
    let actual = metric_delta(before, after, metric_name) / successful_cycles as f64;
    if actual > max_value {
        Some(format!(
            "[raft-metrics-guard] {label} per cycle {actual:.3} above {env_name} {max_value:.3}"
        ))
    } else {
        None
    }
}

fn format_metric_delta(value: f64) -> String {
    if (value.fract()).abs() < 0.000_001 {
        format!("{value:+.0}")
    } else {
        format!("{value:+.3}")
    }
}

fn format_metric_rate(value: f64) -> String {
    if value.abs() < 0.000_001 {
        "0".into()
    } else if (value.fract()).abs() < 0.000_001 {
        format!("{value:.0}")
    } else {
        format!("{value:.3}")
    }
}

fn percentile_ms(values: &[u64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let idx = ((values.len() - 1) as f64 * p).round() as usize;
    values[idx] as f64 / 1000.0
}

fn bench_key(prefix: &str, idx: usize) -> String {
    format!("lmx-bench-{prefix}-{idx}")
}

fn next_key(state: &mut u64, keys: usize) -> usize {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1);
    ((*state >> 32) as usize) % keys
}

fn env_string(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_parse<T>(key: &str, default: T) -> T
where
    T: std::str::FromStr,
{
    env_string(key)
        .and_then(|value| value.parse::<T>().ok())
        .unwrap_or(default)
}

fn env_parse_optional_non_negative_f64(key: &'static str) -> Option<f64> {
    let value = env_string(key)?;
    Some(parse_non_negative_f64_env_value(key, &value))
}

fn parse_non_negative_f64_env_value(key: &'static str, value: &str) -> f64 {
    let parsed = value
        .parse::<f64>()
        .unwrap_or_else(|_| panic!("{key} must be a non-negative finite number; got {value:?}"));
    if !parsed.is_finite() || parsed < 0.0 {
        panic!("{key} must be a non-negative finite number; got {value:?}");
    }
    parsed
}

fn env_bool(key: &'static str, default: bool) -> bool {
    match env_string(key) {
        Some(value) => {
            parse_bool_env_value(key, &value).unwrap_or_else(|message| panic!("{message}"))
        }
        None => default,
    }
}

fn parse_bool_env_value(key: &'static str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" => Ok(true),
        "0" | "false" | "no" | "n" | "off" => Ok(false),
        _ => Err(format!(
            "{key} must be true/false, 1/0, yes/no, y/n, or on/off; got {value:?}"
        )),
    }
}

fn target_env_name(target: &str) -> &'static str {
    match target {
        "redis" => "REDIS",
        "broker" => "BROKER",
        "raft" => "RAFT",
        _ => "UNKNOWN",
    }
}

fn print_usage() {
    println!(
        "Rough Redis / Broker / BrokerRaft acquire+release benchmark.\n\
\n\
Configuration is environment driven:\n\
  BENCH_TARGET=redis|broker|raft|broker-raft|redis-raft|all\n\
  BENCH_REDIS=127.0.0.1:6379\n\
  BENCH_BROKER=127.0.0.1:6971\n\
  BENCH_RAFT=127.0.0.1:6972[,127.0.0.1:6973,...]\n\
  BENCH_RAFT_METRICS_ENDPOINTS=<defaults to BENCH_RAFT>\n\
  BENCH_RAFT_ROUTE=round-robin|leader\n\
  BENCH_WORKERS=8\n\
  BENCH_KEYS=128\n\
  BENCH_DURATION_MS=10000\n\
  BENCH_TTL_MS=5000\n\
  BENCH_IO_TIMEOUT_MS=5000\n\
  BENCH_HTTP_AUTH_TOKEN=<token>\n\
  BENCH_HTTP_KEEPALIVE=false\n\
  BENCH_RAFT_METRICS=false\n\
  BENCH_RAFT_FAIL_ON_FULL_LOG=true\n\
  BENCH_FAIL_ON_ERRORS=true\n\
  BENCH_FAIL_ON_ZERO_SUCCESS=true\n\
  BENCH_MIN_REDIS_OPS_PER_SEC=<optional floor>\n\
  BENCH_MIN_BROKER_OPS_PER_SEC=<optional floor>\n\
  BENCH_MIN_RAFT_OPS_PER_SEC=<optional floor>\n\
  BENCH_MAX_REDIS_P99_MS=<optional ceiling>\n\
  BENCH_MAX_BROKER_P99_MS=<optional ceiling>\n\
  BENCH_MAX_RAFT_P99_MS=<optional ceiling>\n\
  BENCH_MIN_RAFT_CLIENT_BATCH_ENTRIES_PER_BATCH=<optional floor; requires BENCH_RAFT_METRICS=true>\n\
  BENCH_MAX_RAFT_COMMIT_SLOT_WRITES_PER_CYCLE=<optional ceiling; requires BENCH_RAFT_METRICS=true>\n\
  BENCH_MAX_RAFT_LOG_APPEND_WRITE_US_PER_CYCLE=<optional ceiling; requires BENCH_RAFT_METRICS=true>\n\
  BENCH_MAX_RAFT_COMMIT_SLOT_WRITE_US_PER_CYCLE=<optional ceiling; requires BENCH_RAFT_METRICS=true>\n\
\n\
Example:\n\
  BENCH_TARGET=broker-raft BENCH_BROKER=127.0.0.1:6971 BENCH_RAFT=127.0.0.1:6972 \\\n\
    cargo run --release --no-default-features --example redis_vs_raft_bench"
    );
}

async fn http_status(
    endpoint: &str,
    method: &str,
    path: &str,
    auth_token: Option<&str>,
    io_timeout: Duration,
) -> Result<u16, String> {
    let (status, _) = timeout(
        io_timeout,
        http_request(endpoint, method, path, None, auth_token),
    )
    .await
    .map_err(|_| format!("HTTP {method} {path} to {endpoint} timed out after {io_timeout:?}"))??;
    Ok(status)
}

async fn http_request(
    endpoint: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    auth_token: Option<&str>,
) -> Result<(u16, String), String> {
    let (host, port) = parse_host_port(endpoint)?;
    let body = body
        .map(|value| serde_json::to_vec(&value).expect("JSON encode"))
        .unwrap_or_default();
    let auth = auth_token
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Connection: close\r\n\
         Content-Type: application/json\r\n\
         {auth}\
         Content-Length: {}\r\n\
         \r\n",
        body.len()
    );

    let mut stream = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|err| format!("connect {endpoint}: {err}"))?;
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| err.to_string())?;
    stream
        .write_all(&body)
        .await
        .map_err(|err| err.to_string())?;
    stream.flush().await.map_err(|err| err.to_string())?;

    let mut raw = String::new();
    stream
        .read_to_string(&mut raw)
        .await
        .map_err(|err| err.to_string())?;
    let status = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("missing HTTP status line: {raw:?}"))?;
    let body = raw
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

struct HttpConn {
    host: String,
    reader: BufReader<TcpStream>,
}

impl HttpConn {
    async fn connect(endpoint: &str, io_timeout: Duration) -> Result<Self, String> {
        let (host, port) = parse_host_port(endpoint)?;
        let stream = timeout(io_timeout, TcpStream::connect((host.as_str(), port)))
            .await
            .map_err(|_| format!("connect {endpoint} timed out after {io_timeout:?}"))?
            .map_err(|err| format!("connect {endpoint}: {err}"))?;
        Ok(Self {
            host,
            reader: BufReader::new(stream),
        })
    }

    async fn request(
        &mut self,
        method: &str,
        path: &str,
        body: Option<Value>,
        auth_token: Option<&str>,
        io_timeout: Duration,
    ) -> Result<(u16, String), String> {
        let body = body
            .map(|value| serde_json::to_vec(&value).expect("JSON encode"))
            .unwrap_or_default();
        let auth = auth_token
            .map(|token| format!("Authorization: Bearer {token}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "{method} {path} HTTP/1.1\r\n\
             Host: {}\r\n\
             Connection: keep-alive\r\n\
             Content-Type: application/json\r\n\
             {auth}\
             Content-Length: {}\r\n\
             \r\n",
            self.host,
            body.len()
        );

        timeout(io_timeout, async {
            self.reader
                .get_mut()
                .write_all(request.as_bytes())
                .await
                .map_err(|err| err.to_string())?;
            self.reader
                .get_mut()
                .write_all(&body)
                .await
                .map_err(|err| err.to_string())?;
            self.reader
                .get_mut()
                .flush()
                .await
                .map_err(|err| err.to_string())?;
            read_http_response(&mut self.reader).await
        })
        .await
        .map_err(|_| format!("HTTP {method} {path} timed out after {io_timeout:?}"))?
    }
}

async fn read_http_response(reader: &mut BufReader<TcpStream>) -> Result<(u16, String), String> {
    let mut status_line = String::new();
    let mut header_bytes = reader
        .read_line(&mut status_line)
        .await
        .map_err(|err| err.to_string())?;
    if header_bytes == 0 {
        return Err("connection closed before HTTP status line".into());
    }
    if header_bytes > MAX_HTTP_RESPONSE_HEADER_BYTES {
        return Err(format!(
            "HTTP response headers exceeded {MAX_HTTP_RESPONSE_HEADER_BYTES} bytes"
        ));
    }
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("missing HTTP status line: {status_line:?}"))?;

    let mut content_length = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(|err| err.to_string())?;
        if read == 0 {
            return Err("connection closed before HTTP response headers ended".into());
        }
        header_bytes = header_bytes
            .checked_add(read)
            .ok_or_else(|| "HTTP response header byte count overflowed".to_string())?;
        if header_bytes > MAX_HTTP_RESPONSE_HEADER_BYTES {
            return Err(format!(
                "HTTP response headers exceeded {MAX_HTTP_RESPONSE_HEADER_BYTES} bytes"
            ));
        }
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                let len = value
                    .trim()
                    .parse::<usize>()
                    .map_err(|err| format!("invalid Content-Length {value:?}: {err}"))?;
                if len > MAX_HTTP_RESPONSE_BODY_BYTES {
                    return Err(format!(
                        "HTTP response body length {len} exceeds {MAX_HTTP_RESPONSE_BODY_BYTES} bytes"
                    ));
                }
                content_length = Some(len);
            } else if name.eq_ignore_ascii_case("transfer-encoding")
                && value
                    .split(',')
                    .any(|part| part.trim().eq_ignore_ascii_case("chunked"))
            {
                chunked = true;
            }
        }
    }

    let body = if chunked {
        read_chunked_body(reader).await?
    } else {
        let len = content_length
            .ok_or_else(|| "keep-alive HTTP response is missing Content-Length".to_string())?;
        let mut bytes = vec![0u8; len];
        reader
            .read_exact(&mut bytes)
            .await
            .map_err(|err| err.to_string())?;
        bytes
    };
    String::from_utf8(body)
        .map(|body| (status, body))
        .map_err(|err| err.to_string())
}

async fn read_chunked_body(reader: &mut BufReader<TcpStream>) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
        let mut size_line = String::new();
        let read = reader
            .read_line(&mut size_line)
            .await
            .map_err(|err| err.to_string())?;
        if read == 0 {
            return Err("connection closed before chunk size".into());
        }
        let size_hex = size_line
            .trim_end_matches(['\r', '\n'])
            .split_once(';')
            .map(|(size, _)| size)
            .unwrap_or_else(|| size_line.trim_end_matches(['\r', '\n']));
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|err| format!("invalid chunk size {size_hex:?}: {err}"))?;
        if size == 0 {
            let mut trailer_bytes = 0usize;
            loop {
                let mut trailer = String::new();
                let read = reader
                    .read_line(&mut trailer)
                    .await
                    .map_err(|err| err.to_string())?;
                if read == 0 {
                    return Err("connection closed before chunk trailers ended".into());
                }
                trailer_bytes = trailer_bytes
                    .checked_add(read)
                    .ok_or_else(|| "HTTP chunk trailer byte count overflowed".to_string())?;
                if trailer_bytes > MAX_HTTP_RESPONSE_HEADER_BYTES {
                    return Err(format!(
                        "HTTP chunk trailers exceeded {MAX_HTTP_RESPONSE_HEADER_BYTES} bytes"
                    ));
                }
                if trailer.trim_end_matches(['\r', '\n']).is_empty() {
                    return Ok(body);
                }
            }
        }
        if body.len().saturating_add(size) > MAX_HTTP_RESPONSE_BODY_BYTES {
            return Err(format!(
                "chunked HTTP response body exceeds {MAX_HTTP_RESPONSE_BODY_BYTES} bytes"
            ));
        }
        let read_len = size
            .checked_add(2)
            .ok_or_else(|| "chunk size overflowed".to_string())?;
        let mut chunk = vec![0u8; read_len];
        reader
            .read_exact(&mut chunk)
            .await
            .map_err(|err| err.to_string())?;
        if !chunk.ends_with(b"\r\n") {
            return Err("chunk body missing CRLF terminator".into());
        }
        body.extend_from_slice(&chunk[..size]);
    }
}

fn parse_host_port(endpoint: &str) -> Result<(String, u16), String> {
    let endpoint = endpoint
        .strip_prefix("http://")
        .unwrap_or(endpoint)
        .trim_end_matches('/');
    let (host, port) = endpoint
        .rsplit_once(':')
        .ok_or_else(|| format!("endpoint must be host:port, got {endpoint:?}"))?;
    let port = port
        .parse::<u16>()
        .map_err(|_| format!("invalid port in endpoint {endpoint:?}"))?;
    Ok((host.to_string(), port))
}

struct RedisConn {
    reader: BufReader<TcpStream>,
}

impl RedisConn {
    async fn connect(addr: &str, io_timeout: Duration) -> Result<Self, String> {
        let stream = timeout(io_timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| format!("connect {addr} timed out after {io_timeout:?}"))?
            .map_err(|err| format!("connect {addr}: {err}"))?;
        Ok(Self {
            reader: BufReader::new(stream),
        })
    }

    async fn command(&mut self, args: &[&str], io_timeout: Duration) -> Result<RedisValue, String> {
        let encoded = encode_resp(args);
        timeout(io_timeout, async {
            self.reader
                .get_mut()
                .write_all(&encoded)
                .await
                .map_err(|err| err.to_string())?;
            self.reader
                .get_mut()
                .flush()
                .await
                .map_err(|err| err.to_string())?;
            read_redis_value(&mut self.reader).await
        })
        .await
        .map_err(|_| format!("Redis command timed out after {io_timeout:?}"))?
    }
}

fn encode_resp(args: &[&str]) -> Vec<u8> {
    let mut out = format!("*{}\r\n", args.len()).into_bytes();
    for arg in args {
        out.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
        out.extend_from_slice(arg.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out
}

async fn read_redis_value(reader: &mut BufReader<TcpStream>) -> Result<RedisValue, String> {
    let mut prefix = [0u8; 1];
    reader
        .read_exact(&mut prefix)
        .await
        .map_err(|err| err.to_string())?;
    match prefix[0] {
        b'+' => Ok(RedisValue::Simple(read_line(reader).await?)),
        b'-' => Ok(RedisValue::Error(read_line(reader).await?)),
        b':' => read_line(reader)
            .await?
            .parse::<i64>()
            .map(RedisValue::Integer)
            .map_err(|err| err.to_string()),
        b'$' => {
            let len = read_line(reader)
                .await?
                .parse::<isize>()
                .map_err(|err| err.to_string())?;
            if len < 0 {
                return Ok(RedisValue::Bulk(None));
            }
            let mut data = vec![0u8; len as usize + 2];
            reader
                .read_exact(&mut data)
                .await
                .map_err(|err| err.to_string())?;
            data.truncate(len as usize);
            Ok(RedisValue::Bulk(Some(data)))
        }
        b'*' => {
            let len = read_line(reader)
                .await?
                .parse::<usize>()
                .map_err(|err| err.to_string())?;
            for _ in 0..len {
                let _ = Box::pin(read_redis_value(reader)).await?;
            }
            Ok(RedisValue::Array)
        }
        other => Err(format!("unknown Redis response prefix byte {other}")),
    }
}

async fn read_line(reader: &mut BufReader<TcpStream>) -> Result<String, String> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|err| err.to_string())?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_broker_and_raft_endpoints_are_distinct() {
        assert_eq!(DEFAULT_BROKER_ADDR, "127.0.0.1:6971");
        assert_eq!(DEFAULT_RAFT_ADDR, "127.0.0.1:6972");
        assert_ne!(
            DEFAULT_BROKER_ADDR, DEFAULT_RAFT_ADDR,
            "default broker-vs-raft runs must not point both targets at the same HTTP service"
        );
    }

    #[test]
    fn parse_target_accepts_documented_aliases() {
        assert_eq!(parse_target("redis").unwrap(), Target::Redis);
        assert_eq!(parse_target("broker").unwrap(), Target::Broker);
        assert_eq!(parse_target("raft").unwrap(), Target::Raft);
        assert_eq!(parse_target("broker-raft").unwrap(), Target::BrokerRaft);
        assert_eq!(parse_target("brokervsraft").unwrap(), Target::BrokerRaft);
        assert_eq!(parse_target("broker_vs_raft").unwrap(), Target::BrokerRaft);
        assert_eq!(parse_target("redis-raft").unwrap(), Target::RedisRaft);
        assert_eq!(parse_target("redis_vs_raft").unwrap(), Target::RedisRaft);
        assert_eq!(parse_target("both").unwrap(), Target::RedisRaft);
        assert_eq!(parse_target(DEFAULT_TARGET).unwrap(), Target::RedisRaft);
        assert_eq!(parse_target("all").unwrap(), Target::All);
        assert!(parse_target("brokerraft").is_err());
    }

    #[test]
    fn parse_raft_route_accepts_lb_and_leader_modes() {
        assert_eq!(
            parse_raft_route("round-robin").unwrap(),
            RaftRoute::RoundRobin
        );
        assert_eq!(
            parse_raft_route("round_robin").unwrap(),
            RaftRoute::RoundRobin
        );
        assert_eq!(parse_raft_route("rr").unwrap(), RaftRoute::RoundRobin);
        assert_eq!(parse_raft_route("lb").unwrap(), RaftRoute::RoundRobin);
        assert_eq!(parse_raft_route("leader").unwrap(), RaftRoute::Leader);
        assert_eq!(
            parse_raft_route("leader-preferred").unwrap(),
            RaftRoute::Leader
        );
        assert_eq!(
            parse_raft_route("leader_preferred").unwrap(),
            RaftRoute::Leader
        );
        assert!(parse_raft_route("primary").is_err());
    }

    #[test]
    fn parse_bool_env_value_accepts_documented_forms() {
        assert!(parse_bool_env_value("BENCH_HTTP_KEEPALIVE", "TRUE").unwrap());
        assert!(parse_bool_env_value("BENCH_HTTP_KEEPALIVE", "yes").unwrap());
        assert!(parse_bool_env_value("BENCH_HTTP_KEEPALIVE", "y").unwrap());
        assert!(parse_bool_env_value("BENCH_HTTP_KEEPALIVE", "on").unwrap());
        assert!(!parse_bool_env_value("BENCH_RAFT_METRICS", "FALSE").unwrap());
        assert!(!parse_bool_env_value("BENCH_RAFT_METRICS", "0").unwrap());
        assert!(!parse_bool_env_value("BENCH_RAFT_METRICS", "n").unwrap());
        assert!(!parse_bool_env_value("BENCH_RAFT_METRICS", "off").unwrap());
    }

    #[test]
    fn parse_bool_env_value_rejects_typos_instead_of_defaulting_false() {
        let message = parse_bool_env_value("BENCH_RAFT_METRICS", "treu")
            .expect_err("boolean env typo should be rejected");

        assert!(message.contains("BENCH_RAFT_METRICS"));
        assert!(message.contains("treu"));
    }

    #[test]
    fn parse_perf_threshold_accepts_non_negative_finite_values() {
        assert_eq!(
            parse_non_negative_f64_env_value("BENCH_MIN_RAFT_OPS_PER_SEC", "0"),
            0.0
        );
        assert_eq!(
            parse_non_negative_f64_env_value("BENCH_MAX_RAFT_P99_MS", "12.5"),
            12.5
        );
    }

    #[test]
    #[should_panic(expected = "BENCH_MIN_RAFT_OPS_PER_SEC must be a non-negative finite number")]
    fn parse_perf_threshold_rejects_negative_values() {
        let _ = parse_non_negative_f64_env_value("BENCH_MIN_RAFT_OPS_PER_SEC", "-1");
    }

    #[test]
    #[should_panic(expected = "BENCH_MAX_RAFT_P99_MS must be a non-negative finite number")]
    fn parse_perf_threshold_rejects_non_finite_values() {
        let _ = parse_non_negative_f64_env_value("BENCH_MAX_RAFT_P99_MS", "NaN");
    }

    #[test]
    fn summary_throughput_zero_duration_is_zero_not_nan() {
        let summary = Summary {
            ok: 7,
            ..Summary::default()
        };

        assert_eq!(summary.throughput(Duration::ZERO), 0.0);
    }

    #[test]
    fn perf_guard_reports_throughput_latency_and_error_failures() {
        let config = Config {
            redis_addr: DEFAULT_REDIS_ADDR.to_string(),
            broker_addr: DEFAULT_BROKER_ADDR.to_string(),
            raft_addrs: vec![DEFAULT_RAFT_ADDR.to_string()],
            raft_metric_addrs: vec![DEFAULT_RAFT_ADDR.to_string()],
            raft_route: RaftRoute::RoundRobin,
            workers: 1,
            keys: 1,
            duration: Duration::from_secs(2),
            ttl_ms: 5_000,
            io_timeout: Duration::from_millis(DEFAULT_IO_TIMEOUT_MS),
            target: Target::Raft,
            auth_token: None,
            http_keep_alive: false,
            capture_raft_metrics: false,
            fail_on_raft_full_log: true,
            fail_on_errors: true,
            fail_on_zero_success: true,
            perf_thresholds: PerfThresholds {
                min_raft_ops_per_sec: Some(10.0),
                max_raft_p99_ms: Some(1.0),
                ..PerfThresholds::default()
            },
        };
        let summary = Summary {
            ok: 4,
            errors: 1,
            latencies_us: vec![2_000, 2_000, 2_000, 2_000],
            ..Summary::default()
        };

        let failures = config.perf_guard_failures("raft", &summary);

        assert_eq!(failures.len(), 3);
        assert!(failures
            .iter()
            .any(|failure| failure.contains("reported 1 worker/request error")));
        assert!(failures
            .iter()
            .any(|failure| failure.contains("BENCH_MIN_RAFT_OPS_PER_SEC")));
        assert!(failures
            .iter()
            .any(|failure| failure.contains("BENCH_MAX_RAFT_P99_MS")));
    }

    #[test]
    fn raft_metric_guard_reports_batch_efficiency_failures() {
        let thresholds = PerfThresholds {
            min_raft_client_batch_entries_per_batch: Some(4.0),
            max_raft_commit_slot_writes_per_cycle: Some(0.5),
            max_raft_log_append_write_us_per_cycle: Some(100.0),
            max_raft_commit_slot_write_us_per_cycle: Some(75.0),
            ..PerfThresholds::default()
        };
        let before = MetricSnapshot::default();
        let mut after = MetricSnapshot::default();
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batches_total".into(),
            10.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_entries_total".into(),
            20.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_hard_state_commit_slot_writes_total".into(),
            8.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_append_write_us_total".into(),
            1000.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_hard_state_commit_slot_write_us_total".into(),
            600.0,
        );

        let failures = thresholds.raft_metric_guard_failures(&before, &after, 4);

        assert_eq!(failures.len(), 4);
        assert!(failures
            .iter()
            .any(|failure| failure.contains("BENCH_MIN_RAFT_CLIENT_BATCH_ENTRIES_PER_BATCH")));
        assert!(failures
            .iter()
            .any(|failure| failure.contains("BENCH_MAX_RAFT_COMMIT_SLOT_WRITES_PER_CYCLE")));
        assert!(failures
            .iter()
            .any(|failure| failure.contains("BENCH_MAX_RAFT_LOG_APPEND_WRITE_US_PER_CYCLE")));
        assert!(failures
            .iter()
            .any(|failure| failure.contains("BENCH_MAX_RAFT_COMMIT_SLOT_WRITE_US_PER_CYCLE")));
    }

    #[test]
    fn perf_guard_rejects_zero_success_by_default_but_can_report_only() {
        let mut config = Config {
            redis_addr: DEFAULT_REDIS_ADDR.to_string(),
            broker_addr: DEFAULT_BROKER_ADDR.to_string(),
            raft_addrs: vec![DEFAULT_RAFT_ADDR.to_string()],
            raft_metric_addrs: vec![DEFAULT_RAFT_ADDR.to_string()],
            raft_route: RaftRoute::RoundRobin,
            workers: 1,
            keys: 1,
            duration: Duration::from_secs(1),
            ttl_ms: 5_000,
            io_timeout: Duration::from_millis(DEFAULT_IO_TIMEOUT_MS),
            target: Target::Raft,
            auth_token: None,
            http_keep_alive: false,
            capture_raft_metrics: false,
            fail_on_raft_full_log: true,
            fail_on_errors: true,
            fail_on_zero_success: true,
            perf_thresholds: PerfThresholds::default(),
        };
        let summary = Summary::default();

        let failures = config.perf_guard_failures("raft", &summary);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("BENCH_FAIL_ON_ZERO_SUCCESS=false"));

        config.fail_on_zero_success = false;
        assert!(config.perf_guard_failures("raft", &summary).is_empty());
    }

    #[test]
    fn parse_host_port_accepts_plain_or_http_endpoint() {
        assert_eq!(
            parse_host_port("127.0.0.1:6972").unwrap(),
            ("127.0.0.1".to_string(), 6972)
        );
        assert_eq!(
            parse_host_port("http://localhost:6972/").unwrap(),
            ("localhost".to_string(), 6972)
        );
    }

    #[test]
    fn parse_endpoint_list_trims_commas_and_defaults_when_empty() {
        assert_eq!(
            parse_endpoint_list("127.0.0.1:6972, 127.0.0.1:6973,,127.0.0.1:6974"),
            vec![
                "127.0.0.1:6972".to_string(),
                "127.0.0.1:6973".to_string(),
                "127.0.0.1:6974".to_string(),
            ]
        );
        assert_eq!(
            parse_endpoint_list(" , "),
            vec![DEFAULT_RAFT_ADDR.to_string()]
        );
    }

    #[test]
    fn parse_metric_endpoint_list_defaults_to_configured_raft_endpoints() {
        let configured = vec![
            "127.0.0.1:6972".to_string(),
            "127.0.0.1:6973".to_string(),
            "127.0.0.1:6974".to_string(),
        ];

        assert_eq!(
            parse_endpoint_list_with_default(" , ", &configured),
            configured
        );
        assert_eq!(
            parse_endpoint_list_with_default("127.0.0.1:7999", &configured),
            vec!["127.0.0.1:7999".to_string()]
        );
    }

    #[test]
    fn endpoint_for_round_robins_by_worker_and_http_request() {
        let endpoints = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(endpoint_for(&endpoints, 0, 1), "a");
        assert_eq!(endpoint_for(&endpoints, 0, 2), "b");
        assert_eq!(endpoint_for(&endpoints, 0, 3), "c");
        assert_eq!(endpoint_for(&endpoints, 1, 1), "b");
        assert_eq!(endpoint_for(&endpoints, 2, 1), "c");
    }

    #[test]
    fn endpoints_for_cycle_can_split_acquire_and_release() {
        let endpoints = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(endpoints_for_cycle(&endpoints, 0, 1), ("a", "b"));
        assert_eq!(endpoints_for_cycle(&endpoints, 0, 2), ("c", "a"));
        assert_eq!(endpoints_for_cycle(&endpoints, 1, 1), ("b", "c"));

        let single = vec!["leader".to_string()];
        assert_eq!(
            endpoints_for_cycle(&single, 4, 99),
            ("leader", "leader"),
            "single endpoint mode remains leader-preferred"
        );
    }

    #[test]
    fn parse_bool_accepts_common_env_values() {
        assert!(parse_bool_env_value("BENCH_HTTP_KEEPALIVE", "true").unwrap());
        assert!(parse_bool_env_value("BENCH_HTTP_KEEPALIVE", "1").unwrap());
        assert!(parse_bool_env_value("BENCH_HTTP_KEEPALIVE", "on").unwrap());
        assert!(!parse_bool_env_value("BENCH_HTTP_KEEPALIVE", "false").unwrap());
        assert!(!parse_bool_env_value("BENCH_HTTP_KEEPALIVE", "0").unwrap());
        assert!(!parse_bool_env_value("BENCH_HTTP_KEEPALIVE", "off").unwrap());
        assert!(parse_bool_env_value("BENCH_HTTP_KEEPALIVE", "maybe").is_err());
    }

    #[test]
    fn parse_prometheus_metrics_ignores_comments_and_sums_labeled_series() {
        let metrics = parse_prometheus_metrics(
            "\
# HELP ignored help line\n\
dd_rust_network_mutex_raft_append_entries_requests_total 4\n\
histogram_bucket{le=\"1\"} 2\n\
histogram_bucket{le=\"2\"} 3\n\
bad_without_value\n\
bad_value nope\n\
nan_value NaN\n",
        );

        assert_eq!(
            metrics["dd_rust_network_mutex_raft_append_entries_requests_total"],
            4.0
        );
        assert_eq!(metrics["histogram_bucket"], 5.0);
        assert!(!metrics.contains_key("bad_without_value"));
        assert!(!metrics.contains_key("bad_value"));
        assert!(!metrics.contains_key("nan_value"));
    }

    #[test]
    fn raft_metric_delta_lines_report_selected_benchmark_counters() {
        let mut before = MetricSnapshot {
            successful_endpoints: 3,
            ..MetricSnapshot::default()
        };
        before.values.insert(
            "dd_rust_network_mutex_raft_append_entries_requests_total".into(),
            10.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_proxy_requests_forwarded_total".into(),
            1.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_append_entries_frame_size_mismatches_total".into(),
            0.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_admission_quorum_probes_total".into(),
            4.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_admission_quorum_probe_successes_total".into(),
            4.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_admission_quorum_probe_failures_total".into(),
            0.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_admission_quorum_probe_acks_total".into(),
            8.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_admission_quorum_probe_us_total".into(),
            1000.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batches_total".into(),
            2.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_entries_total".into(),
            8.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_pipeline_batches_total".into(),
            3.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_queue_wait_us_total".into(),
            100.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_refill_rounds_total".into(),
            1.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_refilled_entries_total".into(),
            2.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_commit_lock_waits_total".into(),
            2.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_commit_lock_wait_us_total".into(),
            200.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_cancelled_requests_total".into(),
            0.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_errors_total".into(),
            0.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_follower_append_conflicts_total".into(),
            3.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_follower_append_rewrites_total".into(),
            2.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_follower_append_appended_entries_total".into(),
            11.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_hard_state_commit_slot_writes_total".into(),
            2.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_hard_state_commit_slot_write_bytes_total".into(),
            2048.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_hard_state_commit_slot_write_us_total".into(),
            3000.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_append_write_us_total".into(),
            4000.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_compaction_failures_total".into(),
            1.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_compaction_trim_failures_total".into(),
            1.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_reads_total".into(),
            2.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_failures_total".into(),
            0.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_bytes_total".into(),
            2048.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_entries_total".into(),
            12.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrites_total".into(),
            1.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrite_failures_total".into(),
            0.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrite_bytes_total".into(),
            1024.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrite_entries_total".into(),
            4.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_install_snapshot_frame_size_mismatches_total".into(),
            1.0,
        );
        before
            .values
            .insert("dd_rust_network_mutex_raft_log_bytes".into(), 2048.5);

        let mut after = MetricSnapshot {
            successful_endpoints: 3,
            ..MetricSnapshot::default()
        };
        after.values.insert(
            "dd_rust_network_mutex_raft_append_entries_requests_total".into(),
            42.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_proxy_requests_forwarded_total".into(),
            5.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_append_entries_frame_size_mismatches_total".into(),
            2.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_admission_quorum_probes_total".into(),
            14.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_admission_quorum_probe_successes_total".into(),
            13.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_admission_quorum_probe_failures_total".into(),
            1.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_admission_quorum_probe_acks_total".into(),
            28.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_admission_quorum_probe_us_total".into(),
            3500.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batches_total".into(),
            7.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_entries_total".into(),
            38.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_pipeline_batches_total".into(),
            10.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_queue_wait_us_total".into(),
            1900.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_refill_rounds_total".into(),
            3.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_refilled_entries_total".into(),
            11.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_commit_lock_waits_total".into(),
            7.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_commit_lock_wait_us_total".into(),
            2900.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_cancelled_requests_total".into(),
            1.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_errors_total".into(),
            2.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_follower_append_conflicts_total".into(),
            7.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_follower_append_rewrites_total".into(),
            3.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_follower_append_appended_entries_total".into(),
            29.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_hard_state_commit_slot_writes_total".into(),
            8.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_hard_state_commit_slot_write_bytes_total".into(),
            8192.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_hard_state_commit_slot_write_us_total".into(),
            7000.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_append_write_us_total".into(),
            10_000.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_compaction_failures_total".into(),
            3.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_compaction_trim_failures_total".into(),
            2.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_reads_total".into(),
            3.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_failures_total".into(),
            1.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_bytes_total".into(),
            4096.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_entries_total".into(),
            17.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrites_total".into(),
            3.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrite_failures_total".into(),
            1.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrite_bytes_total".into(),
            3072.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrite_entries_total".into(),
            11.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_install_snapshot_frame_size_mismatches_total".into(),
            4.0,
        );
        after
            .values
            .insert("dd_rust_network_mutex_raft_log_bytes".into(), 4096.75);

        let joined = raft_metric_delta_lines(&before, &after).join("\n");

        assert!(joined.contains("sampled_endpoints_before=3 sampled_endpoints_after=3"));
        assert!(joined.contains("append_rpc=+32"));
        assert!(joined.contains("proxy_forwarded=+4"));
        assert!(joined.contains("append_frame_mismatches=+2"));
        assert!(joined.contains("admission_probes=+10"));
        assert!(joined.contains("admission_probe_success=+9"));
        assert!(joined.contains("admission_probe_fail=+1"));
        assert!(joined.contains("admission_probe_acks=+20"));
        assert!(joined.contains("admission_probe_us=+2500"));
        assert!(joined.contains("client_batches=+5"));
        assert!(joined.contains("client_batch_entries=+30"));
        assert!(joined.contains("client_pipeline_batches=+7"));
        assert!(joined.contains("client_queue_wait_us=+1800"));
        assert!(joined.contains("client_refill_rounds=+2"));
        assert!(joined.contains("client_refilled_entries=+9"));
        assert!(joined.contains("client_commit_waits=+5"));
        assert!(joined.contains("client_commit_wait_us=+2700"));
        assert!(joined.contains("client_cancelled=+1"));
        assert!(joined.contains("client_batch_errors=+2"));
        assert!(joined.contains("follower_conflicts=+4"));
        assert!(joined.contains("follower_rewrites=+1"));
        assert!(joined.contains("follower_appended=+18"));
        assert!(joined.contains("commit_slot_writes=+6"));
        assert!(joined.contains("commit_slot_bytes=+6144"));
        assert!(joined.contains("commit_slot_write_us=+4000"));
        assert!(joined.contains("log_append_write_us=+6000"));
        assert!(joined.contains("snapshot_frame_mismatches=+3"));
        assert!(joined.contains("compaction_failures=+2"));
        assert!(joined.contains("compaction_trim_failures=+1"));
        assert!(joined.contains("full_log_reads=+1"));
        assert!(joined.contains("full_log_read_failures=+1"));
        assert!(joined.contains("full_log_read_bytes=+2048"));
        assert!(joined.contains("full_log_read_entries=+5"));
        assert!(joined.contains("full_log_rewrites=+2"));
        assert!(joined.contains("full_log_rewrite_failures=+1"));
        assert!(joined.contains("full_log_rewrite_bytes=+2048"));
        assert!(joined.contains("full_log_rewrite_entries=+7"));
        assert!(joined.contains("log_bytes=+2048.250"));
    }

    #[test]
    fn raft_metric_per_cycle_lines_report_efficiency_ratios() {
        let mut before = MetricSnapshot::default();
        before.values.insert(
            "dd_rust_network_mutex_raft_append_entries_requests_total".into(),
            10.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_append_entries_sent_total".into(),
            20.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_append_entries_log_bytes_total".into(),
            1000.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_admission_quorum_probes_total".into(),
            5.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_admission_quorum_probe_us_total".into(),
            1200.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batches_total".into(),
            2.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_entries_total".into(),
            8.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_pipeline_batches_total".into(),
            3.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_queue_wait_us_total".into(),
            400.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_refill_rounds_total".into(),
            1.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_refilled_entries_total".into(),
            2.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_commit_lock_waits_total".into(),
            2.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_client_batch_commit_lock_wait_us_total".into(),
            800.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_follower_append_appended_entries_total".into(),
            12.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_follower_append_rewrites_total".into(),
            2.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_proxy_requests_forwarded_total".into(),
            2.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_hard_state_commit_slot_writes_total".into(),
            5.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_hard_state_commit_slot_write_bytes_total".into(),
            5120.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_hard_state_commit_slot_write_us_total".into(),
            1000.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_append_write_us_total".into(),
            2000.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_reads_total".into(),
            1.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_failures_total".into(),
            0.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_bytes_total".into(),
            2048.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_entries_total".into(),
            5.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrites_total".into(),
            1.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrite_failures_total".into(),
            0.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrite_bytes_total".into(),
            1024.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrite_entries_total".into(),
            2.0,
        );

        let mut after = MetricSnapshot::default();
        after.values.insert(
            "dd_rust_network_mutex_raft_append_entries_requests_total".into(),
            16.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_append_entries_sent_total".into(),
            32.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_append_entries_log_bytes_total".into(),
            1402.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_admission_quorum_probes_total".into(),
            13.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_admission_quorum_probe_us_total".into(),
            2200.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batches_total".into(),
            10.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_entries_total".into(),
            32.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_pipeline_batches_total".into(),
            11.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_queue_wait_us_total".into(),
            2400.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_refill_rounds_total".into(),
            3.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_refilled_entries_total".into(),
            10.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_commit_lock_waits_total".into(),
            10.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_client_batch_commit_lock_wait_us_total".into(),
            2800.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_follower_append_appended_entries_total".into(),
            32.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_follower_append_rewrites_total".into(),
            6.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_proxy_requests_forwarded_total".into(),
            8.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_hard_state_commit_slot_writes_total".into(),
            13.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_hard_state_commit_slot_write_bytes_total".into(),
            13312.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_hard_state_commit_slot_write_us_total".into(),
            3000.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_append_write_us_total".into(),
            6000.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_reads_total".into(),
            1.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_failures_total".into(),
            0.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_bytes_total".into(),
            2048.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_entries_total".into(),
            5.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrites_total".into(),
            1.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrite_failures_total".into(),
            0.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrite_bytes_total".into(),
            1024.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrite_entries_total".into(),
            2.0,
        );

        let joined = raft_metric_per_cycle_lines(&before, &after, 4).join("\n");

        assert!(joined.contains("[raft-metrics-per-cycle] cycles=4"));
        assert!(joined.contains("append_rpc=1.500"));
        assert!(joined.contains("append_entries=3"));
        assert!(joined.contains("append_log_bytes=100.500"));
        assert!(joined.contains("admission_probe=2"));
        assert!(joined.contains("admission_probe_us=250"));
        assert!(joined.contains("client_batches=2"));
        assert!(joined.contains("client_batch_entries=6"));
        assert!(joined.contains("client_pipeline_batches=2"));
        assert!(joined.contains("client_queue_wait_us=500"));
        assert!(joined.contains("client_refill_rounds=0.500"));
        assert!(joined.contains("client_refilled_entries=2"));
        assert!(joined.contains("client_commit_waits=2"));
        assert!(joined.contains("client_commit_wait_us=500"));
        assert!(joined.contains("follower_appended=5"));
        assert!(joined.contains("follower_rewrites=1"));
        assert!(joined.contains("proxy_forwarded=1.500"));
        assert!(joined.contains("commit_slot_writes=2"));
        assert!(joined.contains("commit_slot_bytes=2048"));
        assert!(joined.contains("commit_slot_write_us=500"));
        assert!(joined.contains("log_append_write_us=1000"));
        assert!(joined.contains("full_log_reads=0"));
        assert!(joined.contains("full_log_read_failures=0"));
        assert!(joined.contains("full_log_read_bytes=0"));
        assert!(joined.contains("full_log_read_entries=0"));
        assert!(joined.contains("full_log_rewrites=0"));
        assert!(joined.contains("full_log_rewrite_failures=0"));
        assert!(joined.contains("full_log_rewrite_bytes=0"));
        assert!(joined.contains("full_log_rewrite_entries=0"));
    }

    #[test]
    fn raft_metric_per_cycle_lines_explain_zero_success_case() {
        let line =
            raft_metric_per_cycle_lines(&MetricSnapshot::default(), &MetricSnapshot::default(), 0)
                .join("\n");

        assert!(line.contains("unavailable because raft completed 0 successful cycles"));
    }

    #[test]
    fn raft_full_log_guard_ignores_unchanged_startup_baseline() {
        let mut before = MetricSnapshot::default();
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_reads_total".into(),
            3.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrites_total".into(),
            1.0,
        );
        let after = MetricSnapshot {
            values: before.values.clone(),
            ..MetricSnapshot::default()
        };

        assert!(raft_full_log_guard_failures(&before, &after).is_empty());
    }

    #[test]
    fn raft_full_log_guard_reports_positive_full_log_deltas() {
        let mut before = MetricSnapshot::default();
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_reads_total".into(),
            2.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_bytes_total".into(),
            1024.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_failures_total".into(),
            0.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrites_total".into(),
            1.0,
        );
        before.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrite_entries_total".into(),
            4.0,
        );

        let mut after = MetricSnapshot::default();
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_reads_total".into(),
            3.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_bytes_total".into(),
            4096.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_read_failures_total".into(),
            1.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrites_total".into(),
            1.0,
        );
        after.values.insert(
            "dd_rust_network_mutex_raft_log_full_rewrite_entries_total".into(),
            6.0,
        );

        let joined = raft_full_log_guard_failures(&before, &after).join("\n");

        assert!(joined.contains("full-log activity observed"));
        assert!(joined.contains("full_log_reads=+1"));
        assert!(joined.contains("full_log_read_failures=+1"));
        assert!(joined.contains("full_log_read_bytes=+3072"));
        assert!(joined.contains("full_log_rewrite_entries=+2"));
        assert!(!joined.contains("full_log_rewrites="));
    }

    #[tokio::test]
    async fn raft_metric_snapshot_uses_metric_endpoints_not_request_endpoints() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind metrics server");
        let metrics_endpoint = listener.local_addr().expect("metrics addr").to_string();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept metrics scrape");
            let mut buf = [0u8; 512];
            let n = stream.read(&mut buf).await.expect("read scrape request");
            let request = String::from_utf8_lossy(&buf[..n]);
            assert!(request.starts_with("GET /metrics "));
            let body = b"dd_rust_network_mutex_raft_log_full_reads_total 5\n";
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream
                .write_all(header.as_bytes())
                .await
                .expect("write metrics header");
            stream.write_all(body).await.expect("write metrics body");
            stream.shutdown().await.expect("shutdown metrics server");
        });
        let config = Config {
            redis_addr: DEFAULT_REDIS_ADDR.to_string(),
            broker_addr: DEFAULT_BROKER_ADDR.to_string(),
            raft_addrs: vec!["127.0.0.1:9".to_string()],
            raft_metric_addrs: vec![metrics_endpoint],
            raft_route: RaftRoute::Leader,
            workers: 1,
            keys: 1,
            duration: Duration::from_secs(1),
            ttl_ms: 5_000,
            io_timeout: Duration::from_secs(1),
            target: Target::Raft,
            auth_token: None,
            http_keep_alive: false,
            capture_raft_metrics: true,
            fail_on_raft_full_log: true,
            fail_on_errors: true,
            fail_on_zero_success: true,
            perf_thresholds: PerfThresholds::default(),
        };

        let snapshot = capture_raft_metric_snapshot(&config, "test").await;

        assert_eq!(snapshot.successful_endpoints, 1);
        assert!(snapshot.errors.is_empty(), "{:?}", snapshot.errors);
        assert_eq!(
            snapshot
                .values
                .get("dd_rust_network_mutex_raft_log_full_reads_total")
                .copied(),
            Some(5.0)
        );
        server.await.expect("metrics server");
    }

    #[tokio::test]
    async fn keep_alive_http_client_reuses_one_socket_for_multiple_json_requests() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test HTTP server");
        let endpoint = listener.local_addr().expect("server addr").to_string();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept HTTP client");
            let mut reader = BufReader::new(stream);
            let mut paths = Vec::new();
            for idx in 0..2 {
                let (path, body) = read_test_http_request(&mut reader).await;
                paths.push((path, body));
                let response = json!({"ok": true, "idx": idx});
                let body = serde_json::to_vec(&response).expect("response JSON");
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                    body.len()
                );
                reader
                    .get_mut()
                    .write_all(header.as_bytes())
                    .await
                    .expect("write response header");
                reader
                    .get_mut()
                    .write_all(&body)
                    .await
                    .expect("write response body");
                reader.get_mut().flush().await.expect("flush response");
            }
            paths
        });

        let mut client = HttpWorkerClient::new(true, None, Duration::from_secs(1));
        let (_, first) = client
            .json(&endpoint, "POST", "/first", Some(json!({"n": 1})))
            .await
            .expect("first keep-alive request");
        let (_, second) = client
            .json(&endpoint, "POST", "/second", Some(json!({"n": 2})))
            .await
            .expect("second keep-alive request");

        assert_eq!(first["idx"], 0);
        assert_eq!(second["idx"], 1);
        assert_eq!(client.connections.len(), 1);
        let paths = server.await.expect("server task");
        assert_eq!(paths[0], ("/first".to_string(), r#"{"n":1}"#.to_string()));
        assert_eq!(paths[1], ("/second".to_string(), r#"{"n":2}"#.to_string()));
    }

    #[tokio::test]
    async fn keep_alive_http_client_reads_chunked_json_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test HTTP server");
        let endpoint = listener.local_addr().expect("server addr").to_string();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept HTTP client");
            let mut reader = BufReader::new(stream);
            let _ = read_test_http_request(&mut reader).await;
            let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n7\r\n{\"ok\":t\r\n4\r\nrue}\r\n0\r\n\r\n";
            reader
                .get_mut()
                .write_all(response)
                .await
                .expect("write chunked response");
            reader.get_mut().flush().await.expect("flush response");
            stream = reader.into_inner();
            stream.shutdown().await.expect("shutdown server stream");
        });

        let mut client = HttpWorkerClient::new(true, None, Duration::from_secs(1));
        let (_, response) = client
            .json(&endpoint, "POST", "/chunked", Some(json!({"n": 1})))
            .await
            .expect("chunked keep-alive request");

        assert_eq!(response["ok"], true);
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn keep_alive_http_client_rejects_oversized_content_length() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test HTTP server");
        let endpoint = listener.local_addr().expect("server addr").to_string();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept HTTP client");
            let mut reader = BufReader::new(stream);
            let _ = read_test_http_request(&mut reader).await;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                MAX_HTTP_RESPONSE_BODY_BYTES + 1
            );
            reader
                .get_mut()
                .write_all(header.as_bytes())
                .await
                .expect("write oversized response header");
            reader.get_mut().flush().await.expect("flush response");
            stream = reader.into_inner();
            stream.shutdown().await.expect("shutdown server stream");
        });

        let mut client = HttpWorkerClient::new(true, None, Duration::from_secs(1));
        let err = client
            .json(&endpoint, "POST", "/oversized", Some(json!({"n": 1})))
            .await
            .expect_err("oversized HTTP body must be rejected before allocation");

        assert!(err.contains("exceeds"), "unexpected error: {err}");
        assert!(
            client.connections.is_empty(),
            "failed keep-alive request should drop the suspect connection"
        );
        server.await.expect("server task");
    }

    async fn read_test_http_request(reader: &mut BufReader<TcpStream>) -> (String, String) {
        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .await
            .expect("read request line");
        let path = request_line
            .split_whitespace()
            .nth(1)
            .expect("request path")
            .to_string();
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("read header");
            let header = line.trim_end_matches(['\r', '\n']);
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.trim().parse().expect("content length");
                }
            }
        }
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).await.expect("read body");
        (path, String::from_utf8(body).expect("request body utf8"))
    }
}
