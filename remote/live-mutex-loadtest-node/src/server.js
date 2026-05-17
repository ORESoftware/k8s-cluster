'use strict';

const http = require('node:http');
const { spawn } = require('node:child_process');
const path = require('node:path');

const { loadConfig, parseLockBackend, parsePositiveInteger } = require('./config');

const config = loadConfig();
const host = process.env.HTTP_HOST || '0.0.0.0';
const port = parsePositiveInteger('HTTP_PORT', 8110);
const mainPath = path.join(__dirname, 'main.js');
const comparePath = path.join(__dirname, 'compare.js');
const maxLogLines = parsePositiveInteger('RUN_LOG_LINES', 200);

let activeRun = null;
let lastRun = null;
let runCounter = 0;

function writeJson(response, statusCode, body) {
  response.writeHead(statusCode, {
    'content-type': 'application/json; charset=utf-8',
    'cache-control': 'no-store',
  });
  response.end(`${JSON.stringify(body, null, 2)}\n`);
}

function collectBody(request) {
  return new Promise((resolve, reject) => {
    let body = '';
    request.setEncoding('utf8');
    request.on('data', (chunk) => {
      body += chunk;
      if (body.length > 64 * 1024) {
        reject(new Error('request body too large'));
        request.destroy();
      }
    });
    request.on('end', () => {
      if (!body.trim()) {
        resolve({});
        return;
      }
      try {
        resolve(JSON.parse(body));
      } catch (error) {
        reject(new Error(`invalid JSON body: ${error.message}`));
      }
    });
    request.on('error', reject);
  });
}

function cleanString(value, fallback) {
  return typeof value === 'string' && value.trim() ? value.trim() : fallback;
}

function cleanPositiveInt(value, fallback) {
  const parsed = Number.parseInt(String(value ?? ''), 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function cleanNonNegativeInt(value, fallback) {
  const parsed = Number.parseInt(String(value ?? ''), 10);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : fallback;
}

function appendLog(run, chunk, stream) {
  const lines = chunk
    .toString('utf8')
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => ({ stream, line }));

  run.logs.push(...lines);
  if (run.logs.length > maxLogLines) {
    run.logs.splice(0, run.logs.length - maxLogLines);
  }
}

function runSnapshot(run) {
  if (!run) {
    return null;
  }
  return {
    id: run.id,
    mode: run.mode,
    backend: run.backend,
    status: run.status,
    startedAt: run.startedAt,
    finishedAt: run.finishedAt || null,
    exitCode: run.exitCode ?? null,
    signal: run.signal || null,
    config: run.publicConfig,
    logs: run.logs,
  };
}

function envForRun(body, mode, backend) {
  const durationSeconds = cleanPositiveInt(
    body.durationSeconds,
    cleanPositiveInt(process.env.DEFAULT_TEST_DURATION_SECONDS, 60),
  );
  const requestsPerSecond = cleanPositiveInt(body.requestsPerSecond, config.requestsPerSecond);
  const workerProcesses = Math.max(3, cleanPositiveInt(body.workerProcesses, config.workerProcesses));
  const clientsPerWorker = cleanPositiveInt(body.clientsPerWorker, config.clientsPerWorker);
  const lockKeys = Array.isArray(body.lockKeys)
    ? body.lockKeys.map((value) => String(value).trim()).filter(Boolean).join(',')
    : cleanString(body.lockKeys, config.lockKeys.join(','));
  const lockHoldMs = cleanNonNegativeInt(body.lockHoldMs, config.lockHoldMs);

  const env = {
    ...process.env,
    BROKER_HOST: cleanString(body.brokerHost, config.brokerHost),
    BROKER_PORT: String(cleanPositiveInt(body.brokerPort, config.brokerPort)),
    CLIENTS_PER_WORKER: String(clientsPerWorker),
    LOCK_HOLD_MS: String(lockHoldMs),
    LOCK_KEYS: lockKeys,
    LOCK_MAX_RETRIES: String(cleanNonNegativeInt(body.lockMaxRetries, config.lockMaxRetries)),
    LOCK_REQUEST_TIMEOUT_MS: String(
      cleanPositiveInt(body.lockRequestTimeoutMs, config.lockRequestTimeoutMs),
    ),
    LOCK_TTL_MS: String(cleanPositiveInt(body.lockTtlMs, config.lockTtlMs)),
    MAX_IN_FLIGHT_PER_WORKER: String(
      cleanPositiveInt(body.maxInFlightPerWorker, config.maxInFlightPerWorker),
    ),
    REDIS_DATABASE: String(cleanNonNegativeInt(body.redisDatabase, config.redisDatabase)),
    REDIS_HOST: cleanString(body.redisHost, config.redisHost),
    REDIS_LOCK_PREFIX: cleanString(body.redisLockPrefix, config.redisLockPrefix),
    REDIS_LOCK_RETRY_DELAY_MS: String(
      cleanPositiveInt(body.redisRetryDelayMs, config.redisRetryDelayMs),
    ),
    REDIS_PASSWORD: cleanString(body.redisPassword, config.redisPassword),
    REDIS_PORT: String(cleanPositiveInt(body.redisPort, config.redisPort)),
    REPORT_INTERVAL_SECONDS: String(
      cleanPositiveInt(body.reportIntervalSeconds, config.reportIntervalSeconds),
    ),
    REQUESTS_PER_SECOND: String(requestsPerSecond),
    TEST_DURATION_SECONDS: String(durationSeconds),
    UNLOCK_REQUEST_TIMEOUT_MS: String(
      cleanPositiveInt(body.unlockRequestTimeoutMs, config.unlockRequestTimeoutMs),
    ),
    WORKER_PROCESSES: String(workerProcesses),
  };

  if (mode === 'compare') {
    env.COMPARE_DURATION_SECONDS = String(durationSeconds);
    env.COMPARE_PAUSE_SECONDS = String(cleanNonNegativeInt(body.comparePauseSeconds, 5));
  } else {
    env.LOCK_BACKEND = backend;
  }

  return {
    env,
    publicConfig: {
      durationSeconds,
      requestsPerSecond,
      workerProcesses,
      clientsPerWorker,
      lockKeys: lockKeys.split(',').filter(Boolean),
      lockHoldMs,
      brokerHost: env.BROKER_HOST,
      brokerPort: Number.parseInt(env.BROKER_PORT, 10),
      redisHost: env.REDIS_HOST,
      redisPort: Number.parseInt(env.REDIS_PORT, 10),
      redisDatabase: Number.parseInt(env.REDIS_DATABASE, 10),
      redisLockPrefix: env.REDIS_LOCK_PREFIX,
    },
  };
}

function startRun(body) {
  if (activeRun) {
    const error = new Error('a load test is already running');
    error.statusCode = 409;
    throw error;
  }

  const requestedMode = cleanString(body.mode, cleanString(body.backend, 'compare'));
  const mode = requestedMode === 'compare' ? 'compare' : 'single';
  const backend = mode === 'compare' ? 'compare' : parseLockBackend(requestedMode);
  const { env, publicConfig } = envForRun(body, mode, backend);
  const child = spawn(process.execPath, [mode === 'compare' ? comparePath : mainPath], {
    env,
    stdio: ['ignore', 'pipe', 'pipe'],
  });

  const run = {
    id: `lock-loadtest-${Date.now()}-${++runCounter}`,
    mode,
    backend,
    status: 'running',
    startedAt: new Date().toISOString(),
    finishedAt: null,
    exitCode: null,
    signal: null,
    publicConfig,
    logs: [],
    child,
  };

  activeRun = run;
  child.stdout.on('data', (chunk) => appendLog(run, chunk, 'stdout'));
  child.stderr.on('data', (chunk) => appendLog(run, chunk, 'stderr'));
  child.on('exit', (code, signal) => {
    run.status = code === 0 ? 'succeeded' : 'failed';
    run.finishedAt = new Date().toISOString();
    run.exitCode = code;
    run.signal = signal;
    lastRun = run;
    activeRun = null;
  });
  child.on('error', (error) => {
    appendLog(run, Buffer.from(error.stack || error.message), 'stderr');
  });

  return run;
}

async function handleRequest(request, response) {
  const url = new URL(request.url || '/', `http://${request.headers.host || 'localhost'}`);

  if (request.method === 'GET' && (url.pathname === '/' || url.pathname === '/healthz')) {
    writeJson(response, 200, {
      service: 'dd-lock-loadtest-trigger',
      status: 'ok',
      activeRun: runSnapshot(activeRun),
      lastRun: runSnapshot(lastRun),
      endpoints: {
        start: 'POST /runs',
        active: 'GET /runs/active',
        last: 'GET /runs/last',
      },
    });
    return;
  }

  if (request.method === 'GET' && url.pathname === '/runs/active') {
    writeJson(response, activeRun ? 200 : 404, { run: runSnapshot(activeRun) });
    return;
  }

  if (request.method === 'GET' && url.pathname === '/runs/last') {
    writeJson(response, lastRun ? 200 : 404, { run: runSnapshot(lastRun) });
    return;
  }

  if (request.method === 'POST' && url.pathname === '/runs') {
    try {
      const body = await collectBody(request);
      const run = startRun(body);
      writeJson(response, 202, { run: runSnapshot(run) });
    } catch (error) {
      writeJson(response, error.statusCode || 400, { error: error.message });
    }
    return;
  }

  writeJson(response, 404, { error: 'not found' });
}

const server = http.createServer((request, response) => {
  void handleRequest(request, response).catch((error) => {
    writeJson(response, 500, { error: error.stack || error.message || String(error) });
  });
});

server.listen(port, host, () => {
  console.log(`dd-lock-loadtest-trigger listening host=${host} port=${port}`);
});
