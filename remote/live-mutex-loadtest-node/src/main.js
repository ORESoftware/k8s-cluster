'use strict';

const { fork } = require('node:child_process');
const path = require('node:path');

const { distributeRate, loadConfig } = require('./config');

const config = loadConfig();
const workerRates = distributeRate(config.requestsPerSecond, config.workerProcesses);
const workerPath = path.join(__dirname, 'worker.js');
const workerStats = new Map();
const children = new Map();

let shuttingDown = false;

function serializeConfigForEnv(workerId, workerRate) {
  return {
    ...process.env,
    BROKER_HOST: config.brokerHost,
    BROKER_PORT: String(config.brokerPort),
    CLIENTS_PER_WORKER: String(config.clientsPerWorker),
    CONNECT_RETRY_DELAY_MS: String(config.connectRetryDelayMs),
    LIVE_MUTEX_WORKER_ID: String(workerId),
    LIVE_MUTEX_WORKER_RPS: String(workerRate),
    LOCK_BACKEND: config.lockBackend,
    LOCK_HOLD_MS: String(config.lockHoldMs),
    LOCK_KEYS: config.lockKeys.join(','),
    LOCK_MAX_RETRIES: String(config.lockMaxRetries),
    LOCK_REQUEST_TIMEOUT_MS: String(config.lockRequestTimeoutMs),
    LOCK_TTL_MS: String(config.lockTtlMs),
    MAX_IN_FLIGHT_PER_WORKER: String(config.maxInFlightPerWorker),
    REDIS_DATABASE: String(config.redisDatabase),
    REDIS_HOST: config.redisHost,
    REDIS_LOCK_PREFIX: config.redisLockPrefix,
    REDIS_LOCK_RETRY_DELAY_MS: String(config.redisRetryDelayMs),
    REDIS_PASSWORD: config.redisPassword,
    REDIS_PORT: String(config.redisPort),
    REPORT_INTERVAL_SECONDS: String(config.reportIntervalSeconds),
    UNLOCK_REQUEST_TIMEOUT_MS: String(config.unlockRequestTimeoutMs),
  };
}

function spawnWorker(workerId) {
  const workerRate = workerRates[workerId];
  const child = fork(workerPath, [], {
    env: serializeConfigForEnv(workerId, workerRate),
    stdio: ['ignore', 'inherit', 'inherit', 'ipc'],
  });

  children.set(workerId, child);
  child.on('message', (message) => {
    if (message && message.type === 'stats') {
      workerStats.set(workerId, message.stats);
    }
  });

  child.on('exit', (code, signal) => {
    children.delete(workerId);
    if (shuttingDown) {
      return;
    }

    console.error(
      `lock-loadtest-node worker_exit worker_id=${workerId} code=${code} signal=${signal}`,
    );
    setTimeout(() => spawnWorker(workerId), config.connectRetryDelayMs);
  });
}

function aggregateStats() {
  const aggregate = {
    started: 0,
    acquired: 0,
    released: 0,
    failed: 0,
    skipped: 0,
    inFlight: 0,
    latencyTotalMs: 0,
    latencySamples: 0,
    latencyMaxMs: 0,
  };

  for (const stats of workerStats.values()) {
    aggregate.started += stats.started || 0;
    aggregate.acquired += stats.acquired || 0;
    aggregate.released += stats.released || 0;
    aggregate.failed += stats.failed || 0;
    aggregate.skipped += stats.skipped || 0;
    aggregate.inFlight += stats.inFlight || 0;
    aggregate.latencyTotalMs += stats.latencyTotalMs || 0;
    aggregate.latencySamples += stats.latencySamples || 0;
    aggregate.latencyMaxMs = Math.max(aggregate.latencyMaxMs, stats.latencyMaxMs || 0);
  }

  return aggregate;
}

let previousAggregate = aggregateStats();

function reportAggregate() {
  const current = aggregateStats();
  const intervalSeconds = config.reportIntervalSeconds;
  const releasedDelta = current.released - previousAggregate.released;
  const failedDelta = current.failed - previousAggregate.failed;
  const skippedDelta = current.skipped - previousAggregate.skipped;
  const avgLatencyMs =
    current.latencySamples > 0 ? current.latencyTotalMs / current.latencySamples : 0;

  console.log(
    [
      'lock-loadtest-node aggregate',
      `backend=${config.lockBackend}`,
      `target_rps=${config.requestsPerSecond}`,
      `actual_released_rps=${(releasedDelta / intervalSeconds).toFixed(2)}`,
      `failed_rps=${(failedDelta / intervalSeconds).toFixed(2)}`,
      `skipped_rps=${(skippedDelta / intervalSeconds).toFixed(2)}`,
      `workers=${config.workerProcesses}`,
      `worker_pids=${[...children.values()].map((child) => child.pid).join(',')}`,
      `lock_keys=${config.lockKeys.length}`,
      `started=${current.started}`,
      `acquired=${current.acquired}`,
      `released=${current.released}`,
      `failed=${current.failed}`,
      `skipped=${current.skipped}`,
      `in_flight=${current.inFlight}`,
      `avg_latency_ms=${avgLatencyMs.toFixed(2)}`,
      `max_latency_ms=${current.latencyMaxMs.toFixed(2)}`,
    ].join(' '),
  );

  previousAggregate = current;
}

function shutdown() {
  if (shuttingDown) {
    return;
  }

  shuttingDown = true;
  console.log('lock-loadtest-node shutting_down');
  for (const child of children.values()) {
    child.send({ type: 'shutdown' });
    setTimeout(() => child.kill('SIGTERM'), 5000).unref();
  }
  setTimeout(() => process.exit(0), 6500).unref();
}

process.on('SIGINT', shutdown);
process.on('SIGTERM', shutdown);

console.log(
  [
    'lock-loadtest-node starting',
    `backend=${config.lockBackend}`,
    `broker=${config.brokerHost}:${config.brokerPort}`,
    `redis=${config.redisHost}:${config.redisPort}/${config.redisDatabase}`,
    `target_rps=${config.requestsPerSecond}`,
    `worker_processes=${config.workerProcesses}`,
    `worker_rates=${workerRates.join(',')}`,
    `clients_per_worker=${config.clientsPerWorker}`,
    `lock_keys=${config.lockKeys.join(',')}`,
    `lock_hold_ms=${config.lockHoldMs}`,
    `lock_ttl_ms=${config.lockTtlMs}`,
    `lock_request_timeout_ms=${config.lockRequestTimeoutMs}`,
    `unlock_request_timeout_ms=${config.unlockRequestTimeoutMs}`,
    `lock_max_retries=${config.lockMaxRetries}`,
    `test_duration_seconds=${config.testDurationSeconds}`,
  ].join(' '),
);

for (let workerId = 0; workerId < config.workerProcesses; workerId += 1) {
  spawnWorker(workerId);
}

setInterval(reportAggregate, config.reportIntervalSeconds * 1000);

if (config.testDurationSeconds > 0) {
  setTimeout(() => {
    reportAggregate();
    shutdown();
  }, config.testDurationSeconds * 1000);
}
