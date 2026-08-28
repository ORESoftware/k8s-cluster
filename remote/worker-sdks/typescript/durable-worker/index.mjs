import { randomUUID as nodeRandomUUID } from 'node:crypto';

const RETRYABLE_HTTP_STATUSES = new Set([408, 425, 429]);
const DEFAULT_RETRY = Object.freeze({
  maxRetries: 3,
  initialDelayMs: 100,
  maxDelayMs: 2_000,
  multiplier: 2,
});

export class DurableWorkerError extends Error {
  constructor(message, options = {}) {
    super(message, { cause: options.cause });
    this.name = 'DurableWorkerError';
    this.status = options.status ?? null;
    this.code = options.code ?? 'sdk_error';
    this.retryable = options.retryable ?? false;
    this.details = options.details ?? null;
  }
}

export class LeaseLostError extends DurableWorkerError {
  constructor(message = 'the durable worker lease is no longer active', options = {}) {
    super(message, {
      ...options,
      code: options.code ?? 'lease_lost',
      retryable: false,
    });
    this.name = 'LeaseLostError';
  }
}

function assertNonEmpty(value, name) {
  if (typeof value !== 'string' || value.trim() === '') {
    throw new TypeError(`${name} must be a non-empty string`);
  }
  return value;
}

function assertPositiveInteger(value, name) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new TypeError(`${name} must be a positive safe integer`);
  }
  return value;
}

function normalizeBaseUrl(value) {
  const url = new URL(assertNonEmpty(value, 'baseUrl'));
  if (!['http:', 'https:'].includes(url.protocol)) {
    throw new TypeError('baseUrl must use http or https');
  }
  if (url.username || url.password || url.search || url.hash) {
    throw new TypeError('baseUrl must not contain credentials, query, or fragment');
  }
  url.pathname = url.pathname.replace(/\/+$/, '');
  return url.toString().replace(/\/$/, '');
}

function normalizeObject(value) {
  if (value == null) return {};
  if (typeof value === 'object' && !Array.isArray(value)) return value;
  return { value };
}

function retryDelay(retry, attempt) {
  return Math.min(
    retry.maxDelayMs,
    Math.round(retry.initialDelayMs * retry.multiplier ** attempt),
  );
}

function retryAfterMs(response) {
  const raw = response.headers.get('retry-after');
  if (!raw) return null;
  const seconds = Number(raw);
  if (Number.isFinite(seconds) && seconds >= 0) return Math.round(seconds * 1_000);
  const date = Date.parse(raw);
  if (!Number.isNaN(date)) return Math.max(0, date - Date.now());
  return null;
}

function isRetryableStatus(status) {
  return RETRYABLE_HTTP_STATUSES.has(status) || status >= 500;
}

function timeoutError(timeoutMs) {
  const error = new DurableWorkerError(`request timed out after ${timeoutMs}ms`, {
    code: 'request_timeout',
    retryable: true,
  });
  error.name = 'TimeoutError';
  return error;
}

function createAttemptSignal(parentSignal, timeoutMs) {
  const controller = new AbortController();
  let timer = null;
  let parentListener = null;

  if (parentSignal) {
    if (parentSignal.aborted) {
      controller.abort(parentSignal.reason);
    } else {
      parentListener = () => controller.abort(parentSignal.reason);
      parentSignal.addEventListener('abort', parentListener, { once: true });
    }
  }

  if (Number.isFinite(timeoutMs) && timeoutMs > 0) {
    timer = setTimeout(() => controller.abort(timeoutError(timeoutMs)), timeoutMs);
    timer.unref?.();
  }

  return {
    signal: controller.signal,
    cleanup() {
      if (timer) clearTimeout(timer);
      if (parentSignal && parentListener) {
        parentSignal.removeEventListener('abort', parentListener);
      }
    },
  };
}

export function sleep(ms, signal) {
  if (!Number.isFinite(ms) || ms <= 0) return Promise.resolve();
  if (signal?.aborted) return Promise.reject(signal.reason ?? new Error('aborted'));

  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', onAbort);
      resolve();
    }, ms);

    function onAbort() {
      clearTimeout(timer);
      reject(signal.reason ?? new Error('aborted'));
    }
    signal?.addEventListener('abort', onAbort, { once: true });
  });
}

async function parseResponse(response) {
  if (response.status === 204) return undefined;
  const text = await response.text();
  if (text === '') return undefined;
  const contentType = response.headers.get('content-type') ?? '';
  if (contentType.includes('json')) {
    try {
      return JSON.parse(text);
    } catch (cause) {
      throw new DurableWorkerError('the durable worker returned invalid JSON', {
        status: response.status,
        code: 'invalid_json_response',
        retryable: false,
        cause,
        details: text,
      });
    }
  }
  return text;
}

function normalizeRetry(value = {}) {
  const retry = { ...DEFAULT_RETRY, ...value };
  if (!Number.isSafeInteger(retry.maxRetries) || retry.maxRetries < 0) {
    throw new TypeError('retry.maxRetries must be a non-negative safe integer');
  }
  for (const key of ['initialDelayMs', 'maxDelayMs', 'multiplier']) {
    if (!Number.isFinite(retry[key]) || retry[key] <= 0) {
      throw new TypeError(`retry.${key} must be positive`);
    }
  }
  return Object.freeze(retry);
}

function leaseCommand(workerId, assignment) {
  return {
    workerId,
    leaseToken: assignment.leaseToken,
    leaseGeneration: assignment.leaseGeneration,
  };
}

function handlerFor(handlers, taskType) {
  if (handlers instanceof Map) return handlers.get(taskType);
  return handlers?.[taskType];
}

function safeCode(error, fallback) {
  if (typeof error?.code === 'string' && error.code.trim() !== '') return error.code;
  return fallback;
}

function safeMessage(error) {
  if (typeof error?.message === 'string' && error.message.trim() !== '') {
    return error.message;
  }
  return String(error ?? 'worker handler failed');
}

function safeNotify(onError, error, context) {
  if (typeof onError !== 'function') return;
  try {
    onError(error, context);
  } catch {
    // Observability callbacks must never change durable execution semantics.
  }
}

export class DurableWorkerClient {
  constructor(options) {
    if (!options || typeof options !== 'object') {
      throw new TypeError('DurableWorkerClient options are required');
    }
    this.baseUrl = normalizeBaseUrl(options.baseUrl);
    this.authSecret = assertNonEmpty(options.authSecret, 'authSecret');
    if (/[\r\n]/u.test(this.authSecret)) {
      throw new TypeError('authSecret must be a single-line value');
    }
    this.authHeader = (options.authHeader ?? 'x-worker-auth').toLowerCase();
    if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/u.test(this.authHeader)) {
      throw new TypeError('authHeader must be a valid HTTP token');
    }
    this.fetch = options.fetch ?? globalThis.fetch;
    if (typeof this.fetch !== 'function') {
      throw new TypeError('a Fetch-compatible implementation is required');
    }
    this.requestTimeoutMs = options.requestTimeoutMs ?? 30_000;
    if (!Number.isFinite(this.requestTimeoutMs) || this.requestTimeoutMs <= 0) {
      throw new TypeError('requestTimeoutMs must be positive');
    }
    this.retry = normalizeRetry(options.retry);
    this.randomUUID = options.randomUUID ?? nodeRandomUUID;
    if (typeof this.randomUUID !== 'function') {
      throw new TypeError('randomUUID must be a function');
    }
  }

  async request(path, options = {}) {
    const method = options.method ?? 'GET';
    const idempotent = options.idempotent ?? ['GET', 'HEAD', 'OPTIONS'].includes(method);
    const timeoutMs = options.timeoutMs ?? this.requestTimeoutMs;
    const maxAttempts = idempotent ? this.retry.maxRetries + 1 : 1;
    const url = new URL(path, `${this.baseUrl}/`).toString();

    for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
      const attemptSignal = createAttemptSignal(options.signal, timeoutMs);
      let response;
      try {
        response = await this.fetch(url, {
          method,
          redirect: 'manual',
          signal: attemptSignal.signal,
          headers: {
            accept: 'application/json',
            [this.authHeader]: this.authSecret,
            ...(options.body === undefined ? {} : { 'content-type': 'application/json' }),
            ...options.headers,
          },
          body: options.body === undefined ? undefined : JSON.stringify(options.body),
        });
      } catch (cause) {
        attemptSignal.cleanup();
        if (options.signal?.aborted) throw options.signal.reason ?? cause;
        const error = cause instanceof DurableWorkerError
          ? cause
          : new DurableWorkerError('durable worker request failed', {
              code: 'network_error',
              retryable: true,
              cause,
            });
        if (attempt + 1 >= maxAttempts) throw error;
        await sleep(retryDelay(this.retry, attempt), options.signal);
        continue;
      }
      attemptSignal.cleanup();

      const body = await parseResponse(response);
      if (response.ok) return body;

      const error = new DurableWorkerError(
        typeof body?.message === 'string'
          ? body.message
          : `durable worker request failed with HTTP ${response.status}`,
        {
          status: response.status,
          code: typeof body?.code === 'string' ? body.code : 'http_error',
          retryable: body?.retryable ?? isRetryableStatus(response.status),
          details: body,
        },
      );
      if (
        attempt + 1 >= maxAttempts
        || !isRetryableStatus(response.status)
        || error.retryable === false
      ) {
        throw error;
      }
      await sleep(retryAfterMs(response) ?? retryDelay(this.retry, attempt), options.signal);
    }

    throw new DurableWorkerError('request retry loop exhausted unexpectedly');
  }

  submitTask(task, options = {}) {
    return this.request('/api/v1/tasks', {
      ...options,
      method: 'POST',
      body: task,
      idempotent: Boolean(task?.idempotencyKey),
    });
  }

  submitRun(run, options = {}) {
    return this.request('/api/v1/runs', {
      ...options,
      method: 'POST',
      body: run,
      idempotent: Boolean(run?.idempotencyKey),
    });
  }

  getRun(runId, options = {}) {
    return this.request(`/api/v1/runs/${encodeURIComponent(assertNonEmpty(runId, 'runId'))}`, options);
  }

  signalRun(runId, signalName, payload = {}, options = {}) {
    return this.request(
      `/api/v1/runs/${encodeURIComponent(assertNonEmpty(runId, 'runId'))}/signals/${encodeURIComponent(assertNonEmpty(signalName, 'signalName'))}`,
      { ...options, method: 'POST', body: { payload: normalizeObject(payload) }, idempotent: false },
    );
  }

  pauseRun(runId, options = {}) {
    return this.#runMutation(runId, 'pause', options);
  }

  resumeRun(runId, options = {}) {
    return this.#runMutation(runId, 'resume', options);
  }

  cancelRun(runId, options = {}) {
    return this.#runMutation(runId, 'cancel', options);
  }

  #runMutation(runId, operation, options) {
    return this.request(
      `/api/v1/runs/${encodeURIComponent(assertNonEmpty(runId, 'runId'))}/${operation}`,
      { ...options, method: 'POST', body: {}, idempotent: true },
    );
  }

  registerWorker(registration, options = {}) {
    return this.request('/api/v1/workers/register', {
      ...options,
      method: 'POST',
      body: registration,
      idempotent: true,
    });
  }

  heartbeatWorker(workerId, heartbeat = {}, options = {}) {
    return this.request(
      `/api/v1/workers/${encodeURIComponent(assertNonEmpty(workerId, 'workerId'))}/heartbeat`,
      { ...options, method: 'POST', body: heartbeat, idempotent: true },
    );
  }

  pollWorker(workerId, poll = {}, options = {}) {
    const waitMs = poll.waitMs ?? 30_000;
    if (!Number.isSafeInteger(waitMs) || waitMs < 0) {
      throw new TypeError('poll.waitMs must be a non-negative safe integer');
    }
    return this.request(
      `/api/v1/workers/${encodeURIComponent(assertNonEmpty(workerId, 'workerId'))}/poll?waitMs=${waitMs}`,
      {
        ...options,
        method: 'POST',
        body: {},
        idempotent: false,
        timeoutMs: Math.max(options.timeoutMs ?? this.requestTimeoutMs, waitMs + 5_000),
      },
    );
  }

  startStep(stepId, command, options = {}) {
    return this.#stepMutation(stepId, 'start', command, options);
  }

  heartbeatStep(stepId, command, options = {}) {
    return this.#stepMutation(stepId, 'heartbeat', command, options);
  }

  completeStep(stepId, command, result = {}, options = {}) {
    return this.#stepMutation(
      stepId,
      'complete',
      { ...command, result: normalizeObject(result) },
      options,
    );
  }

  failStep(stepId, command, failure, options = {}) {
    return this.#stepMutation(
      stepId,
      'fail',
      {
        ...command,
        code: assertNonEmpty(failure?.code, 'failure.code'),
        message: assertNonEmpty(failure?.message, 'failure.message'),
        retryable: failure?.retryable ?? true,
      },
      options,
    );
  }

  appendOutput(stepId, command, output, options = {}) {
    const chunkId = output?.chunkId ?? this.randomUUID();
    return this.#stepMutation(
      stepId,
      'output',
      {
        ...command,
        chunkId: assertNonEmpty(chunkId, 'output.chunkId'),
        stream: output?.stream ?? 'progress',
        chunk: String(output?.chunk ?? ''),
        finalChunk: output?.finalChunk ?? false,
      },
      options,
    );
  }

  #stepMutation(stepId, operation, body, options) {
    return this.request(
      `/api/v1/steps/${encodeURIComponent(assertNonEmpty(stepId, 'stepId'))}/${operation}`,
      { ...options, method: 'POST', body, idempotent: true },
    );
  }

  async runWorker(options) {
    const workerId = assertNonEmpty(options?.workerId, 'workerId');
    const slots = assertPositiveInteger(options?.slots ?? 1, 'slots');
    const ttlMs = assertPositiveInteger(options?.ttlMs ?? 60_000, 'ttlMs');
    const pollWaitMs = options?.pollWaitMs ?? 30_000;
    const workerHeartbeatMs = options?.workerHeartbeatMs ?? Math.max(1_000, Math.floor(ttlMs / 3));
    const maxAssignments = options?.maxAssignments ?? Number.POSITIVE_INFINITY;
    if (
      maxAssignments !== Number.POSITIVE_INFINITY
      && (!Number.isSafeInteger(maxAssignments) || maxAssignments <= 0)
    ) {
      throw new TypeError('maxAssignments must be a positive safe integer or Infinity');
    }
    if (!Number.isFinite(workerHeartbeatMs) || workerHeartbeatMs <= 0) {
      throw new TypeError('workerHeartbeatMs must be positive');
    }

    const externalSignal = options?.signal;
    const heartbeatStop = new AbortController();
    const active = new Set();
    const summary = { accepted: 0, succeeded: 0, failed: 0, leaseLost: 0 };
    let stopPolling = externalSignal?.aborted ?? false;
    const onExternalAbort = () => {
      stopPolling = true;
    };
    externalSignal?.addEventListener('abort', onExternalAbort, { once: true });

    await this.registerWorker(
      {
        workerId,
        queues: [...(options?.queues ?? [])],
        capabilities: [...(options?.capabilities ?? [])],
        labels: normalizeObject(options?.labels),
        slots,
        ttlMs,
        drain: false,
      },
      { signal: externalSignal },
    );

    const heartbeatTask = this.#workerHeartbeatLoop({
      workerId,
      intervalMs: workerHeartbeatMs,
      signal: heartbeatStop.signal,
      onError: options?.onError,
    });

    try {
      while (!stopPolling && summary.accepted < maxAssignments) {
        if (active.size >= slots) {
          await Promise.race(active);
          continue;
        }

        let polled;
        try {
          polled = await this.pollWorker(
            workerId,
            { waitMs: pollWaitMs },
            { signal: externalSignal },
          );
        } catch (error) {
          if (externalSignal?.aborted) break;
          safeNotify(options?.onError, error, { phase: 'poll-ambiguous', workerId });
          // A lost poll response may already contain a leased assignment. Repeating
          // the poll can over-admit work, so stop and let the server expire/redeliver.
          throw error;
        }

        if (!polled?.assignment) {
          const retryAfter = Math.max(0, polled?.retryAfterMs ?? 50);
          if (retryAfter > 0) await sleep(retryAfter, externalSignal);
          continue;
        }

        summary.accepted += 1;
        const assignment = polled.assignment;
        let execution;
        execution = this.#executeAssignment({
          workerId,
          assignment,
          handlers: options?.handlers,
          signal: externalSignal,
          leaseHeartbeatFraction: options?.leaseHeartbeatFraction ?? 0.4,
          onError: options?.onError,
        })
          .then((outcome) => {
            summary[outcome] += 1;
          })
          .finally(() => {
            active.delete(execution);
          });
        active.add(execution);
      }

      await Promise.allSettled(active);
    } finally {
      heartbeatStop.abort(new Error('worker heartbeat loop stopped'));
      await heartbeatTask.catch(() => {});
      if (options?.drainOnStop ?? true) {
        await this.heartbeatWorker(workerId, { drain: true }).catch((error) => {
          safeNotify(options?.onError, error, { phase: 'drain', workerId });
        });
      }
      externalSignal?.removeEventListener('abort', onExternalAbort);
    }

    return summary;
  }

  async #workerHeartbeatLoop({ workerId, intervalMs, signal, onError }) {
    while (!signal.aborted) {
      try {
        await sleep(intervalMs, signal);
        if (signal.aborted) break;
        await this.heartbeatWorker(workerId, { drain: false }, { signal });
      } catch (error) {
        if (signal.aborted) break;
        safeNotify(onError, error, { phase: 'worker-heartbeat', workerId });
        if (!(error instanceof DurableWorkerError) || !error.retryable) break;
      }
    }
  }

  async #executeAssignment({
    workerId,
    assignment,
    handlers,
    signal,
    leaseHeartbeatFraction,
    onError,
  }) {
    if (!Number.isFinite(leaseHeartbeatFraction) || leaseHeartbeatFraction <= 0 || leaseHeartbeatFraction >= 1) {
      throw new TypeError('leaseHeartbeatFraction must be greater than zero and less than one');
    }

    const command = leaseCommand(workerId, assignment);
    const handler = handlerFor(handlers, assignment.taskType);
    if (typeof handler !== 'function') {
      await this.failStep(assignment.stepId, command, {
        code: 'handler_not_found',
        message: `no handler is registered for task type ${assignment.taskType}`,
        retryable: false,
      });
      return 'failed';
    }

    await this.startStep(assignment.stepId, command, { signal });

    const handlerController = new AbortController();
    let externalAbortListener = null;
    if (signal) {
      if (signal.aborted) {
        handlerController.abort(signal.reason);
      } else {
        externalAbortListener = () => handlerController.abort(signal.reason);
        signal.addEventListener('abort', externalAbortListener, { once: true });
      }
    }
    const heartbeatStop = new AbortController();
    let leaseLost = false;
    let leaseLossError = null;
    const initialLeaseRemaining = Math.max(1_000, assignment.leaseExpiresAtMs - Date.now());
    const heartbeatIntervalMs = Math.max(
      250,
      Math.floor(initialLeaseRemaining * leaseHeartbeatFraction),
    );

    const heartbeatTask = (async () => {
      while (!heartbeatStop.signal.aborted) {
        try {
          await sleep(heartbeatIntervalMs, heartbeatStop.signal);
          if (heartbeatStop.signal.aborted) break;
          await this.heartbeatStep(assignment.stepId, command, {
            signal: heartbeatStop.signal,
          });
        } catch (error) {
          if (heartbeatStop.signal.aborted) break;
          if (error instanceof DurableWorkerError && error.status === 409) {
            leaseLost = true;
            leaseLossError = new LeaseLostError(error.message, {
              status: error.status,
              details: error.details,
              cause: error,
            });
            handlerController.abort(leaseLossError);
            break;
          }
          safeNotify(onError, error, {
            phase: 'step-heartbeat',
            workerId,
            assignment,
          });
        }
      }
    })();

    const context = Object.freeze({
      assignment,
      client: this,
      fencingToken: assignment.fencingToken,
      runId: assignment.runId,
      stepId: assignment.stepId,
      signal: handlerController.signal,
      heartbeat: () => this.heartbeatStep(assignment.stepId, command, {
        signal: handlerController.signal,
      }),
      progress: (chunk, output = {}) => this.appendOutput(
        assignment.stepId,
        command,
        {
          ...output,
          chunk: String(chunk),
          chunkId: output.chunkId ?? this.randomUUID(),
        },
        { signal: handlerController.signal },
      ),
    });

    try {
      const result = await handler(assignment.input, context);
      if (leaseLost) throw leaseLossError;
      await this.completeStep(
        assignment.stepId,
        command,
        normalizeObject(result),
        { signal },
      );
      return 'succeeded';
    } catch (error) {
      if (leaseLost || error instanceof LeaseLostError) {
        safeNotify(onError, leaseLossError ?? error, {
          phase: 'lease-lost',
          workerId,
          assignment,
        });
        return 'leaseLost';
      }

      const retryable = error?.retryable !== false;
      try {
        await this.failStep(
          assignment.stepId,
          command,
          {
            code: safeCode(error, signal?.aborted ? 'worker_aborted' : 'handler_error'),
            message: safeMessage(error),
            retryable,
          },
        );
      } catch (failureError) {
        if (failureError instanceof DurableWorkerError && failureError.status === 409) {
          safeNotify(onError, failureError, {
            phase: 'lease-lost-during-failure',
            workerId,
            assignment,
          });
          return 'leaseLost';
        }
        throw failureError;
      }
      safeNotify(onError, error, { phase: 'handler', workerId, assignment });
      return 'failed';
    } finally {
      heartbeatStop.abort(new Error('assignment finished'));
      await heartbeatTask.catch(() => {});
      if (signal && externalAbortListener) {
        signal.removeEventListener('abort', externalAbortListener);
      }
    }
  }
}
