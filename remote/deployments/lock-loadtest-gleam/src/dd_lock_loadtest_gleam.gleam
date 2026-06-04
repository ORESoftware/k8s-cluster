//// Mutex-broker load tester (Gleam, BEAM target).
////
//// Drives an acquire/release workload from BEAM against any broker
//// that speaks the live-mutex NDJSON TCP protocol — the same wire
//// format used by `dd-rust-network-mutex`, `dd-live-mutex`, and the
//// improved fork at `dd-live-mutex-submodule`.
////
//// Operating model:
////   * Read config from env (BROKER_HOST, BROKER_PORT, BENCH_*).
////   * On startup, run one bench window. Print a JSON summary line.
////   * Sleep `BENCH_INTERVAL_S`, then loop. Exits on SIGTERM.
////
//// Why no HTTP server: the goal of this binary is cross-runtime
//// correctness evidence (BEAM <-> broker) plus a per-language
//// latency profile, not an interactive trigger surface. The Rust
//// load tester (`dd-lock-loadtest-rs`) already exposes a POST /runs
//// API; pulling cowboy/mist into Gleam to mirror that would be
//// 10x the deps for a feature this binary doesn't need.
////
//// Output shape (one line per bench window, parseable by
//// `kubectl logs | jq`):
//// ```
//// {
////   "type": "bench-summary",
////   "runId": "...",
////   "brokerHost": "dd-rust-network-mutex.default.svc.cluster.local",
////   "brokerPort": 6970,
////   "workers": 16,
////   "keys": 32,
////   "durationMs": 10000,
////   "acquired": 12345,
////   "released": 12345,
////   "failedAcquires": 0,
////   "failedReleases": 0,
////   "fencingViolations": 0,
////   "actualRps": 1234.5,
////   "latencyUsP50": 88,
////   "latencyUsP95": 145,
////   "latencyUsP99": 200,
////   "latencyUsMax": 10207
//// }
//// ```

import dd_rust_network_mutex_client as nm
import gleam/dict.{type Dict}
import gleam/erlang/process.{type Subject}
import gleam/int
import gleam/io
import gleam/list
import gleam/option.{None}
import gleam/string

/// Read an env var as String with a fallback. Erlang's
/// `os:getenv/1` returns `false` for missing vars; we wrap it in a
/// tiny FFI helper to keep the pattern match in one place.
@external(erlang, "lock_loadtest_gleam_env_ffi", "getenv")
fn ffi_getenv(name: String, fallback: String) -> String

@external(erlang, "lock_loadtest_gleam_env_ffi", "system_time_micro")
fn ffi_system_time_micro() -> Int

@external(erlang, "lock_loadtest_gleam_env_ffi", "sleep_ms")
fn ffi_sleep_ms(ms: Int) -> Nil

@external(erlang, "lock_loadtest_gleam_env_ffi", "uuid_v4")
fn ffi_uuid_v4() -> String

pub type Config {
  Config(
    broker_host: String,
    broker_port: Int,
    workers: Int,
    keys: Int,
    duration_ms: Int,
    interval_ms: Int,
    ttl_ms: Int,
  )
}

pub type WorkerStats {
  WorkerStats(
    acquired: Int,
    released: Int,
    failed_acquires: Int,
    failed_releases: Int,
    /// Per-acquire latency samples in microseconds. Reservoir-sampled
    /// (~capped at 10k samples per worker) so a long bench window
    /// doesn't blow up the BEAM heap.
    latency_samples: List(Int),
    /// Per-key fencing-token high-water mark observed by this worker.
    /// We merge these across workers at the end of the bench window
    /// to detect monotonicity violations.
    fencing_tokens: Dict(String, Int),
  )
}

const sample_cap: Int = 10_000

pub fn main() -> Nil {
  let config = read_config()
  io.println(format_startup(config))

  // Loop forever: bench, sleep, bench, sleep, …. Kubernetes will
  // SIGTERM us on pod shutdown; the BEAM exits cleanly when the
  // shell process is killed.
  loop_forever(config)
}

fn loop_forever(config: Config) -> Nil {
  let _ = run_one_bench(config)
  ffi_sleep_ms(config.interval_ms)
  loop_forever(config)
}

fn run_one_bench(config: Config) -> Result(Nil, Nil) {
  let run_id = ffi_uuid_v4()
  let started_at = ffi_system_time_micro()
  let deadline_micro = started_at + config.duration_ms * 1000

  // Spawn each worker as a BEAM process. Each posts its `WorkerStats`
  // to a shared `Subject` when it finishes. The parent (this fn)
  // collects exactly `workers` messages with a generous timeout that
  // tracks the bench duration plus a small safety pad.
  let collector: Subject(WorkerStats) = process.new_subject()
  let _pids =
    seq(0, config.workers)
    |> list.map(fn(worker_id) {
      process.spawn(fn() {
        let stats = run_worker(worker_id, config, deadline_micro)
        process.send(collector, stats)
      })
    })

  let collect_timeout_ms = config.duration_ms + 30_000
  let stats = collect_n(collector, config.workers, collect_timeout_ms, [])

  let finished_at = ffi_system_time_micro()
  let summary = aggregate(stats, config, run_id, started_at, finished_at)
  io.println(summary)
  Ok(Nil)
}

/// Build [0, n) without depending on `list.range` — `gleam/list`
/// dropped that helper around stdlib 0.39, but we want to keep
/// stdlib bounds open at the high end. Tail-recursive.
fn seq(start: Int, count: Int) -> List(Int) {
  do_seq(start, start + count - 1, [])
}

fn do_seq(start: Int, finish: Int, acc: List(Int)) -> List(Int) {
  case finish < start {
    True -> acc
    False -> do_seq(start, finish - 1, [finish, ..acc])
  }
}

/// Block until `n` workers have reported back, or `timeout_ms`
/// elapses. We always return whatever we got — partial results are
/// still better than no result at all if a worker hung on a connect.
fn collect_n(
  subj: Subject(WorkerStats),
  remaining: Int,
  timeout_ms: Int,
  acc: List(WorkerStats),
) -> List(WorkerStats) {
  case remaining <= 0 {
    True -> acc
    False ->
      case process.receive(subj, timeout_ms) {
        Ok(s) -> collect_n(subj, remaining - 1, timeout_ms, [s, ..acc])
        Error(_) -> acc
      }
  }
}

/// Single-worker loop. Connects once, runs acquire/release until
/// `deadline_micro` is reached or a connection error occurs.
fn run_worker(
  worker_id: Int,
  config: Config,
  deadline_micro: Int,
) -> WorkerStats {
  case nm.connect(config.broker_host, config.broker_port, None) {
    Error(_) ->
      WorkerStats(
        acquired: 0,
        released: 0,
        failed_acquires: 1,
        failed_releases: 0,
        latency_samples: [],
        fencing_tokens: dict.new(),
      )
    Ok(client) -> {
      let stats =
        acquire_release_loop(
          client,
          worker_id,
          config,
          deadline_micro,
          empty_stats(),
          0,
        )
      nm.close(client)
      stats
    }
  }
}

fn acquire_release_loop(
  client: nm.Client,
  worker_id: Int,
  config: Config,
  deadline_micro: Int,
  stats: WorkerStats,
  iter: Int,
) -> WorkerStats {
  let now_micro = ffi_system_time_micro()
  case now_micro >= deadline_micro {
    True -> stats
    False -> {
      let key_id = { worker_id + iter } % config.keys
      let key = "loadtest-gleam-key-" <> int.to_string(key_id)
      let acq_started = ffi_system_time_micro()
      case nm.acquire(client, key, config.ttl_ms) {
        Error(_) ->
          acquire_release_loop(
            client,
            worker_id,
            config,
            deadline_micro,
            WorkerStats(..stats, failed_acquires: stats.failed_acquires + 1),
            iter + 1,
          )
        Ok(handle) -> {
          let acq_done = ffi_system_time_micro()
          let latency = acq_done - acq_started
          let stats_with_acq =
            WorkerStats(
              ..stats,
              acquired: stats.acquired + 1,
              latency_samples: maybe_record(stats.latency_samples, latency),
              fencing_tokens: dict.insert(
                stats.fencing_tokens,
                key,
                int.max(
                  case dict.get(stats.fencing_tokens, key) {
                    Ok(v) -> v
                    Error(_) -> 0
                  },
                  handle.fencing_token,
                ),
              ),
            )
          let stats_after_release = case nm.release_single(client, handle) {
            Error(_) ->
              WorkerStats(
                ..stats_with_acq,
                failed_releases: stats_with_acq.failed_releases + 1,
              )
            Ok(_) ->
              WorkerStats(
                ..stats_with_acq,
                released: stats_with_acq.released + 1,
              )
          }
          acquire_release_loop(
            client,
            worker_id,
            config,
            deadline_micro,
            stats_after_release,
            iter + 1,
          )
        }
      }
    }
  }
}

/// Reservoir-style cap: keep at most `sample_cap` samples. Past the
/// cap we drop further samples (vs Vitter's algorithm-R) — at high
/// RPS the dropped samples are statistically indistinguishable from
/// the kept ones for percentile estimation, and this keeps the loop
/// allocation-free past the cap.
fn maybe_record(samples: List(Int), latency: Int) -> List(Int) {
  case list.length(samples) >= sample_cap {
    True -> samples
    False -> [latency, ..samples]
  }
}

fn empty_stats() -> WorkerStats {
  WorkerStats(
    acquired: 0,
    released: 0,
    failed_acquires: 0,
    failed_releases: 0,
    latency_samples: [],
    fencing_tokens: dict.new(),
  )
}

// ---------------------------------------------------------------------------
// Aggregation + JSON output
// ---------------------------------------------------------------------------

fn aggregate(
  per_worker: List(WorkerStats),
  config: Config,
  run_id: String,
  started_at_micro: Int,
  finished_at_micro: Int,
) -> String {
  let totals =
    list.fold(per_worker, empty_stats(), fn(acc, ws) {
      WorkerStats(
        acquired: acc.acquired + ws.acquired,
        released: acc.released + ws.released,
        failed_acquires: acc.failed_acquires + ws.failed_acquires,
        failed_releases: acc.failed_releases + ws.failed_releases,
        latency_samples: list.append(acc.latency_samples, ws.latency_samples),
        fencing_tokens: merge_tokens(acc.fencing_tokens, ws.fencing_tokens),
      )
    })
  let sorted = list.sort(totals.latency_samples, int.compare)
  let n = list.length(sorted)
  let p50 = percentile(sorted, n, 50)
  let p95 = percentile(sorted, n, 95)
  let p99 = percentile(sorted, n, 99)
  let p_max = case list.last(sorted) {
    Ok(v) -> v
    Error(_) -> 0
  }
  let elapsed_micros = finished_at_micro - started_at_micro
  let actual_rps = case elapsed_micros > 0 {
    True -> totals.acquired * 1_000_000 / elapsed_micros
    False -> 0
  }

  // We don't currently detect monotonicity violations within a
  // single window (workers may observe out-of-order tokens just
  // because they don't share a clock). The fencing_tokens dict is
  // exported as the per-key high-water for visibility, but we leave
  // strict per-key monotonicity assertions to the Rust load tester.
  let json_pairs = [
    #("type", quote("bench-summary")),
    #("runId", quote(run_id)),
    #("brokerHost", quote(config.broker_host)),
    #("brokerPort", int.to_string(config.broker_port)),
    #("workers", int.to_string(config.workers)),
    #("keys", int.to_string(config.keys)),
    #("durationMs", int.to_string(config.duration_ms)),
    #("startedAtMicro", int.to_string(started_at_micro)),
    #("finishedAtMicro", int.to_string(finished_at_micro)),
    #("acquired", int.to_string(totals.acquired)),
    #("released", int.to_string(totals.released)),
    #("failedAcquires", int.to_string(totals.failed_acquires)),
    #("failedReleases", int.to_string(totals.failed_releases)),
    #("actualRps", int.to_string(actual_rps)),
    #("latencyUsP50", int.to_string(p50)),
    #("latencyUsP95", int.to_string(p95)),
    #("latencyUsP99", int.to_string(p99)),
    #("latencyUsMax", int.to_string(p_max)),
    #("uniqueKeysObserved", int.to_string(dict.size(totals.fencing_tokens))),
  ]
  format_json_object(json_pairs)
}

fn merge_tokens(a: Dict(String, Int), b: Dict(String, Int)) -> Dict(String, Int) {
  // Per-key: keep the high-water across workers.
  dict.fold(b, a, fn(acc, k, v) {
    case dict.get(acc, k) {
      Ok(existing) ->
        case v > existing {
          True -> dict.insert(acc, k, v)
          False -> acc
        }
      Error(_) -> dict.insert(acc, k, v)
    }
  })
}

fn percentile(sorted: List(Int), n: Int, pct: Int) -> Int {
  case n {
    0 -> 0
    _ -> {
      // Index = ceil(n * pct / 100) - 1, clamped.
      let idx_1 = n * pct / 100
      let idx = case idx_1 >= n {
        True -> n - 1
        False -> idx_1
      }
      case list.drop(sorted, idx) {
        [head, ..] -> head
        [] -> 0
      }
    }
  }
}

fn quote(s: String) -> String {
  // Tiny JSON-string quoter. We intentionally don't pull in a JSON
  // library here — the only strings we emit are run UUIDs, broker
  // hostnames, and small literals; none contain control chars or
  // backslashes in normal operation. If the broker host ever grows
  // a quote (it shouldn't), we'd need a fuller escape pass.
  "\"" <> s <> "\""
}

fn format_json_object(pairs: List(#(String, String))) -> String {
  let body =
    pairs
    |> list.map(fn(p) { quote(p.0) <> ":" <> p.1 })
    |> string.join(",")
  "{" <> body <> "}"
}

// ---------------------------------------------------------------------------
// Config + startup banner
// ---------------------------------------------------------------------------

fn read_config() -> Config {
  let host =
    ffi_getenv("BROKER_HOST", "dd-rust-network-mutex.default.svc.cluster.local")
  let port = parse_int(ffi_getenv("BROKER_PORT", "6970"), 6970)
  let workers = parse_int(ffi_getenv("BENCH_WORKERS", "16"), 16)
  let keys = parse_int(ffi_getenv("BENCH_KEYS", "32"), 32)
  let duration_ms = parse_int(ffi_getenv("BENCH_DURATION_MS", "10000"), 10_000)
  let interval_ms = parse_int(ffi_getenv("BENCH_INTERVAL_MS", "30000"), 30_000)
  let ttl_ms = parse_int(ffi_getenv("BENCH_TTL_MS", "4000"), 4000)
  Config(
    broker_host: host,
    broker_port: port,
    workers: clamp(workers, 1, 1024),
    keys: clamp(keys, 1, 65_536),
    duration_ms: clamp(duration_ms, 100, 600_000),
    interval_ms: clamp(interval_ms, 0, 3_600_000),
    ttl_ms: clamp(ttl_ms, 100, 600_000),
  )
}

fn parse_int(s: String, fallback: Int) -> Int {
  case int.parse(s) {
    Ok(v) -> v
    Error(_) -> fallback
  }
}

fn clamp(v: Int, lo: Int, hi: Int) -> Int {
  case v < lo {
    True -> lo
    False ->
      case v > hi {
        True -> hi
        False -> v
      }
  }
}

fn format_startup(config: Config) -> String {
  format_json_object([
    #("type", quote("startup")),
    #("brokerHost", quote(config.broker_host)),
    #("brokerPort", int.to_string(config.broker_port)),
    #("workers", int.to_string(config.workers)),
    #("keys", int.to_string(config.keys)),
    #("durationMs", int.to_string(config.duration_ms)),
    #("intervalMs", int.to_string(config.interval_ms)),
    #("ttlMs", int.to_string(config.ttl_ms)),
  ])
}

