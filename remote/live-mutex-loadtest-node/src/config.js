'use strict';

const DEFAULT_LOCK_KEYS = Object.freeze([
  'lmx-loadtest-a',
  'lmx-loadtest-b',
  'lmx-loadtest-c',
  'lmx-loadtest-d',
  'lmx-loadtest-e',
]);

function parseInteger(name, fallback, env, predicate) {
  const raw = env[name];
  if (raw === undefined || raw === '') {
    return fallback;
  }

  const parsed = Number.parseInt(raw, 10);
  return Number.isFinite(parsed) && predicate(parsed) ? parsed : fallback;
}

function parsePositiveInteger(name, fallback, env = process.env) {
  return parseInteger(name, fallback, env, (value) => value > 0);
}

function parseNonNegativeInteger(name, fallback, env = process.env) {
  return parseInteger(name, fallback, env, (value) => value >= 0);
}

function parseLockKeys(rawValue, fallback = DEFAULT_LOCK_KEYS) {
  const rawKeys = String(rawValue || '')
    .split(',')
    .map((key) => key.trim())
    .filter(Boolean);

  const uniqueKeys = [...new Set(rawKeys)];
  return uniqueKeys.length > 0 ? uniqueKeys : [...fallback];
}

function parseLockBackend(rawValue) {
  return rawValue === 'redis' ? 'redis' : 'live-mutex';
}

function distributeRate(totalRate, workerCount) {
  const baseRate = Math.floor(totalRate / workerCount);
  const remainder = totalRate % workerCount;
  return Array.from({ length: workerCount }, (_value, index) =>
    baseRate + (index < remainder ? 1 : 0),
  );
}

function loadConfig(env = process.env) {
  const workerProcesses = Math.max(3, parsePositiveInteger('WORKER_PROCESSES', 3, env));

  return {
    lockBackend: parseLockBackend(env.LOCK_BACKEND),
    brokerHost: env.BROKER_HOST || 'dd-live-mutex.default.svc.cluster.local',
    brokerPort: parsePositiveInteger('BROKER_PORT', 6970, env),
    redisHost: env.REDIS_HOST || 'dd-redis-cache.default.svc.cluster.local',
    redisPort: parsePositiveInteger('REDIS_PORT', 6379, env),
    redisDatabase: parseNonNegativeInteger('REDIS_DATABASE', 0, env),
    redisPassword: env.REDIS_PASSWORD || '',
    redisLockPrefix: env.REDIS_LOCK_PREFIX || 'dd-locktest',
    redisRetryDelayMs: parsePositiveInteger('REDIS_LOCK_RETRY_DELAY_MS', 1, env),
    requestsPerSecond: parsePositiveInteger('REQUESTS_PER_SECOND', 1000, env),
    workerProcesses,
    clientsPerWorker: parsePositiveInteger('CLIENTS_PER_WORKER', 12, env),
    lockKeys: parseLockKeys(env.LOCK_KEYS),
    lockHoldMs: parseNonNegativeInteger('LOCK_HOLD_MS', 0, env),
    lockTtlMs: parsePositiveInteger('LOCK_TTL_MS', 4000, env),
    lockRequestTimeoutMs: parsePositiveInteger('LOCK_REQUEST_TIMEOUT_MS', 3000, env),
    unlockRequestTimeoutMs: parsePositiveInteger('UNLOCK_REQUEST_TIMEOUT_MS', 3000, env),
    lockMaxRetries: parseNonNegativeInteger('LOCK_MAX_RETRIES', 0, env),
    connectRetryDelayMs: parsePositiveInteger('CONNECT_RETRY_DELAY_MS', 1000, env),
    maxInFlightPerWorker: parsePositiveInteger('MAX_IN_FLIGHT_PER_WORKER', 2000, env),
    reportIntervalSeconds: parsePositiveInteger('REPORT_INTERVAL_SECONDS', 10, env),
    testDurationSeconds: parseNonNegativeInteger('TEST_DURATION_SECONDS', 0, env),
  };
}

module.exports = {
  DEFAULT_LOCK_KEYS,
  distributeRate,
  loadConfig,
  parseLockBackend,
  parseLockKeys,
  parseNonNegativeInteger,
  parsePositiveInteger,
};
