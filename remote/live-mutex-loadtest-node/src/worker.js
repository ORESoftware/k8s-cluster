'use strict';

const { Client } = require('live-mutex');
const { createClient } = require('redis');
const crypto = require('node:crypto');

const {
  loadConfig,
  parseNonNegativeInteger,
  parsePositiveInteger,
} = require('./config');

function sleep(ms) {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

function nowMs() {
  const [seconds, nanoseconds] = process.hrtime();
  return seconds * 1000 + nanoseconds / 1e6;
}

function formatError(error) {
  if (!error) {
    return 'unknown';
  }
  return error.code ? `${error.code}:${error.message}` : String(error.message || error);
}

const config = loadConfig();
const workerId = parseNonNegativeInteger('LIVE_MUTEX_WORKER_ID', 0);
const workerRps = parsePositiveInteger(
  'LIVE_MUTEX_WORKER_RPS',
  Math.ceil(config.requestsPerSecond / 3),
);
process.setMaxListeners(Math.max(process.getMaxListeners(), config.clientsPerWorker + 20));

const tickMs = 100;
const requestsPerTick = workerRps * (tickMs / 1000);
const clients = [];
const redisReleaseScript = `
if redis.call("get", KEYS[1]) == ARGV[1] then
  return redis.call("del", KEYS[1])
else
  return 0
end
`;

const stats = {
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

let shuttingDown = false;
let nextClient = 0;
let nextKey = 0;
let scheduleCarry = 0;
let tokenCounter = 0;

async function connectLiveMutexClient(client, index) {
  while (!shuttingDown) {
    try {
      await client.ensure();
      return;
    } catch (error) {
      console.error(
        `lock-loadtest-node worker_connect_failed worker_id=${workerId} client=${index} error=${formatError(error)}`,
      );
      await sleep(config.connectRetryDelayMs);
    }
  }
}

async function connectPool() {
  for (let index = 0; index < config.clientsPerWorker; index += 1) {
    const client =
      config.lockBackend === 'redis'
        ? createClient({
            database: config.redisDatabase,
            password: config.redisPassword || undefined,
            socket: {
              host: config.redisHost,
              port: config.redisPort,
              connectTimeout: config.lockRequestTimeoutMs,
              noDelay: true,
            },
          })
        : new Client({
            host: config.brokerHost,
            port: config.brokerPort,
            lockRequestTimeout: config.lockRequestTimeoutMs,
            unlockRequestTimeout: config.unlockRequestTimeoutMs,
            ttl: config.lockTtlMs,
            noDelay: true,
          });

    if (config.lockBackend === 'redis') {
      client.on('error', (error) => {
        console.error(
          `lock-loadtest-node redis_client_error worker_id=${workerId} client=${index} error=${formatError(error)}`,
        );
      });
    }
    clients.push(client);
  }

  if (config.lockBackend === 'redis') {
    await Promise.all(clients.map((client) => client.connect()));
  } else {
    await Promise.all(clients.map((client, index) => connectLiveMutexClient(client, index)));
  }
}

function redisKey(lockKey) {
  return `${config.redisLockPrefix}:${lockKey}`;
}

function nextToken(lockKey) {
  tokenCounter += 1;
  return `${process.pid}:${workerId}:${lockKey}:${Date.now()}:${tokenCounter}:${crypto.randomUUID()}`;
}

async function acquireRedisLock(client, lockKey, token) {
  const deadline = Date.now() + config.lockRequestTimeoutMs;
  const key = redisKey(lockKey);

  while (!shuttingDown && Date.now() <= deadline) {
    const acquired = await client.set(key, token, {
      NX: true,
      PX: config.lockTtlMs,
    });

    if (acquired === 'OK') {
      return key;
    }

    await sleep(config.redisRetryDelayMs);
  }

  throw new Error(`redis lock request timed out for key ${lockKey}`);
}

async function runLiveMutexLockCycle(client, lockKey) {
  const lock = await client.acquire(lockKey, {
    ttl: config.lockTtlMs,
    lockRequestTimeout: config.lockRequestTimeoutMs,
    maxRetries: config.lockMaxRetries,
  });

  stats.acquired += 1;
  if (config.lockHoldMs > 0) {
    await sleep(config.lockHoldMs);
  }

  await client.release(lock.key || lockKey, {
    id: lock.id || lock.lockUuid,
    unlockRequestTimeout: config.unlockRequestTimeoutMs,
  });
}

async function runRedisLockCycle(client, lockKey) {
  const token = nextToken(lockKey);
  const key = await acquireRedisLock(client, lockKey, token);

  stats.acquired += 1;
  if (config.lockHoldMs > 0) {
    await sleep(config.lockHoldMs);
  }

  const released = await client.eval(redisReleaseScript, {
    keys: [key],
    arguments: [token],
  });

  if (released !== 1) {
    throw new Error(`redis unlock compare-and-delete failed for key ${key}`);
  }
}

async function runLockCycle() {
  if (stats.inFlight >= config.maxInFlightPerWorker) {
    stats.skipped += 1;
    return;
  }

  const client = clients[nextClient];
  const lockKey = config.lockKeys[nextKey];
  nextClient = (nextClient + 1) % clients.length;
  nextKey = (nextKey + 1) % config.lockKeys.length;

  stats.started += 1;
  stats.inFlight += 1;
  const startedAt = nowMs();

  try {
    if (config.lockBackend === 'redis') {
      await runRedisLockCycle(client, lockKey);
    } else {
      await runLiveMutexLockCycle(client, lockKey);
    }

    const latencyMs = nowMs() - startedAt;
    stats.released += 1;
    stats.latencyTotalMs += latencyMs;
    stats.latencySamples += 1;
    stats.latencyMaxMs = Math.max(stats.latencyMaxMs, latencyMs);
  } catch (error) {
    stats.failed += 1;
    console.error(
      `lock-loadtest-node worker_request_failed worker_id=${workerId} key=${lockKey} error=${formatError(error)}`,
    );
  } finally {
    stats.inFlight = Math.max(0, stats.inFlight - 1);
  }
}

function scheduleTick() {
  if (shuttingDown) {
    return;
  }

  scheduleCarry += requestsPerTick;
  const launchCount = Math.floor(scheduleCarry);
  scheduleCarry -= launchCount;

  for (let index = 0; index < launchCount; index += 1) {
    void runLockCycle();
  }
}

function sendStats() {
  if (process.send) {
    process.send({
      type: 'stats',
      stats: {
        ...stats,
        workerId,
        pid: process.pid,
        targetRps: workerRps,
      },
    });
  }
}

process.on('message', (message) => {
  if (message && message.type === 'shutdown') {
    shuttingDown = true;
    setTimeout(() => process.exit(0), 1000).unref();
  }
});

process.on('SIGTERM', () => {
  shuttingDown = true;
  setTimeout(() => process.exit(0), 1000).unref();
});

async function main() {
  console.log(
    [
      'lock-loadtest-node worker_starting',
      `backend=${config.lockBackend}`,
      `worker_id=${workerId}`,
      `pid=${process.pid}`,
      `target_rps=${workerRps}`,
      `broker=${config.brokerHost}:${config.brokerPort}`,
      `redis=${config.redisHost}:${config.redisPort}/${config.redisDatabase}`,
      `clients=${config.clientsPerWorker}`,
      `lock_keys=${config.lockKeys.join(',')}`,
    ].join(' '),
  );

  await connectPool();
  console.log(`lock-loadtest-node worker_ready worker_id=${workerId} pid=${process.pid}`);

  setInterval(scheduleTick, tickMs);
  setInterval(sendStats, config.reportIntervalSeconds * 1000);
}

main().catch((error) => {
  console.error(`lock-loadtest-node worker_fatal worker_id=${workerId} error=${formatError(error)}`);
  process.exit(1);
});
