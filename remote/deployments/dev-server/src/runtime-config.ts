// Receiver for the dd-runtime-config control plane.
//
// Exposes:
//   GET  /internal/runtime-config          — what this process currently has
//   POST /internal/update-runtime-config   — accept a new snapshot
//   POST /internal/runtime-config/reset    — drop all runtime overrides
//
// Mutating routes require `X-Server-Auth: $RUNTIME_CONFIG_SERVER_SECRET`.
// Local unauthenticated development must opt in explicitly with
// `RUNTIME_CONFIG_ALLOW_UNAUTHENTICATED=true`.
//
// At startup the host should call `registerWithControlPlane()` so the control
// plane knows where to push.
//
// Payload shapes match remote/libs/interfaces/shared/schema/runtime-config.schema.json.
// The shared TS interfaces are the canonical reference; dev-server inlines the
// minimal subset here so it stays inside its own tsc rootDir.

import type { FastifyInstance } from 'fastify';
import { timingSafeEqual } from 'node:crypto';

import { contextFetch } from './wrapped-fetch.js';

type RuntimeConfigEnv = 'stage' | 'prod';

type RuntimeConfigEntry = {
  env: RuntimeConfigEnv;
  scope: string;
  key: string;
  value: unknown;
  version: number;
  updatedAt: string;
  labels?: string[];
  meta?: unknown;
};

type RuntimeConfigSnapshot = {
  env: RuntimeConfigEnv;
  scope: string;
  generatedAt: string;
  snapshotVersion: number;
  entries: RuntimeConfigEntry[];
};

type RuntimeConfigApplyRequest = {
  pushId: string;
  reason: 'cron' | 'admin' | 'register' | 'manual' | 'initial';
  snapshot: RuntimeConfigSnapshot;
};

type RuntimeConfigApplyResponse = {
  ok: boolean;
  service: string;
  appliedAt: string;
  appliedVersion: number;
  previousVersion?: number;
  stale?: boolean;
  ignoredVersion?: number;
  errors?: string[];
};

const ENV_SERVICE_NAME = 'RUNTIME_CONFIG_SERVICE_NAME';
const ENV_SCOPE = 'RUNTIME_CONFIG_SCOPE';
const ENV_ENV = 'RUNTIME_CONFIG_ENV';
const ENV_REGISTER_URL = 'RUNTIME_CONFIG_REGISTER_URL';
const ENV_APPLY_URL = 'RUNTIME_CONFIG_APPLY_URL';
const ENV_SERVER_SECRET = 'RUNTIME_CONFIG_SERVER_SECRET';
const ENV_ALLOW_UNAUTHENTICATED = 'RUNTIME_CONFIG_ALLOW_UNAUTHENTICATED';
const REGISTER_BACKOFF_MS = 15_000;
const REGISTER_MAX_BACKOFF_MS = 5 * 60_000;

type StoreState = {
  snapshotVersion: number;
  appliedAt: string | null;
  entries: Map<string, unknown>;
  lastPushId: string | null;
  lastReason: string | null;
};

export class RuntimeConfigStore {
  private state: StoreState = {
    snapshotVersion: 0,
    appliedAt: null,
    entries: new Map(),
    lastPushId: null,
    lastReason: null,
  };

  private readonly serverSecret = readEnv(ENV_SERVER_SECRET);
  private readonly allowUnauthenticated = readBoolEnv(ENV_ALLOW_UNAUTHENTICATED);

  get(key: string): unknown | undefined {
    return this.state.entries.get(key);
  }

  snapshotVersion(): number {
    return this.state.snapshotVersion;
  }

  applySnapshot(payload: RuntimeConfigApplyRequest): RuntimeConfigApplyResponse {
    const previousVersion = this.state.snapshotVersion;
    if (!payload.snapshot || typeof payload.snapshot !== 'object') {
      throw new Error('snapshot is required');
    }
    const newEntries = new Map<string, unknown>();
    const snapshot = payload.snapshot;
    if (snapshot && Array.isArray(snapshot.entries)) {
      for (const entry of snapshot.entries as RuntimeConfigEntry[]) {
        if (typeof entry?.key === 'string') {
          newEntries.set(entry.key, entry.value);
        }
      }
    }
    const appliedVersion = Number.isInteger(snapshot?.snapshotVersion)
      ? Number(snapshot.snapshotVersion)
      : 0;
    if (appliedVersion < previousVersion) {
      return {
        ok: true,
        service: readEnv(ENV_SERVICE_NAME) ?? 'unknown',
        appliedAt: this.state.appliedAt ?? new Date().toISOString(),
        appliedVersion: previousVersion,
        previousVersion,
        stale: true,
        ignoredVersion: appliedVersion,
      };
    }
    this.state = {
      snapshotVersion: appliedVersion,
      appliedAt: new Date().toISOString(),
      entries: newEntries,
      lastPushId: typeof payload?.pushId === 'string' ? payload.pushId : null,
      lastReason: typeof payload?.reason === 'string' ? payload.reason : null,
    };
    return {
      ok: true,
      service: readEnv(ENV_SERVICE_NAME) ?? 'unknown',
      appliedAt: this.state.appliedAt ?? new Date().toISOString(),
      appliedVersion,
      previousVersion,
    };
  }

  reset(): void {
    this.state = {
      snapshotVersion: 0,
      appliedAt: null,
      entries: new Map(),
      lastPushId: null,
      lastReason: null,
    };
  }

  snapshot(): Record<string, unknown> {
    return {
      service: readEnv(ENV_SERVICE_NAME) ?? null,
      scope: readEnv(ENV_SCOPE) ?? null,
      env: readEnv(ENV_ENV) ?? null,
      snapshotVersion: this.state.snapshotVersion,
      appliedAt: this.state.appliedAt,
      entries: Object.fromEntries(this.state.entries),
      lastPushId: this.state.lastPushId,
      lastReason: this.state.lastReason,
    };
  }

  requireServerAuth(authHeader: string | undefined): { ok: true } | { ok: false } {
    if (!this.serverSecret) return this.allowUnauthenticated ? { ok: true } : { ok: false };
    if (typeof authHeader !== 'string' || authHeader.length === 0) return { ok: false };
    const provided = Buffer.from(authHeader);
    const expected = Buffer.from(this.serverSecret);
    if (provided.length !== expected.length || !timingSafeEqual(provided, expected)) {
      return { ok: false };
    }
    return { ok: true };
  }
}

export const runtimeConfigStore = new RuntimeConfigStore();

/**
 * Register the local Fastify routes for the runtime-config receive surface.
 * Call this once, before fastify.listen().
 */
export function registerRuntimeConfigRoutes(fastify: FastifyInstance): void {
  fastify.get('/internal/runtime-config', async () => runtimeConfigStore.snapshot());

  fastify.post('/internal/update-runtime-config', async (req, reply) => {
    const auth = runtimeConfigStore.requireServerAuth(
      typeof req.headers['x-server-auth'] === 'string'
        ? (req.headers['x-server-auth'] as string)
        : undefined,
    );
    if (!auth.ok) {
      return reply.code(401).send({ ok: false, error: 'unauthorized' });
    }
    const payload = req.body as RuntimeConfigApplyRequest | undefined;
    if (!payload || typeof payload !== 'object') {
      return reply.code(400).send({ ok: false, error: 'invalid payload' });
    }
    try {
      return runtimeConfigStore.applySnapshot(payload);
    } catch (error) {
      return reply.code(400).send({
        ok: false,
        error: (error as Error)?.message ?? 'invalid payload',
      });
    }
  });

  fastify.post('/internal/runtime-config/reset', async (req, reply) => {
    const auth = runtimeConfigStore.requireServerAuth(
      typeof req.headers['x-server-auth'] === 'string'
        ? (req.headers['x-server-auth'] as string)
        : undefined,
    );
    if (!auth.ok) {
      return reply.code(401).send({ ok: false, error: 'unauthorized' });
    }
    runtimeConfigStore.reset();
    return { ok: true };
  });
}

/**
 * Background registration with the control plane. Retries with exponential
 * backoff until success. Resolves once registered (used mostly so callers can
 * await registration in tests).
 */
export async function registerWithControlPlane(): Promise<void> {
  const registerUrl = readEnv(ENV_REGISTER_URL);
  const applyUrl = readEnv(ENV_APPLY_URL);
  const serviceName = readEnv(ENV_SERVICE_NAME);
  const envLabel = (readEnv(ENV_ENV) ?? 'stage') as RuntimeConfigEnv;
  const scope = readEnv(ENV_SCOPE) ?? serviceName ?? undefined;

  if (!registerUrl || !applyUrl || !serviceName || !scope) {
    console.warn(
      '[runtime-config] missing one of RUNTIME_CONFIG_REGISTER_URL / _APPLY_URL / _SERVICE_NAME / _SCOPE; skipping registration',
    );
    return;
  }

  const body = {
    env: envLabel,
    name: serviceName,
    scope,
    applyUrl,
  };
  const secret = readEnv(ENV_SERVER_SECRET);

  let delay = REGISTER_BACKOFF_MS;
  while (true) {
    try {
      const response = await contextFetch(registerUrl, {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          ...(secret ? { 'x-server-auth': secret } : {}),
        },
        body: JSON.stringify(body),
      });
      if (response.ok) {
        console.log(`[runtime-config] registered with control plane at ${registerUrl}`);
        return;
      }
      console.warn(
        `[runtime-config] register returned HTTP ${response.status}; retrying in ${Math.round(delay / 1000)}s`,
      );
    } catch (error) {
      console.warn(
        `[runtime-config] register transport error: ${(error as Error)?.message ?? error}; retrying in ${Math.round(delay / 1000)}s`,
      );
    }
    await sleep(delay);
    delay = Math.min(delay * 2, REGISTER_MAX_BACKOFF_MS);
  }
}

function readEnv(name: string): string | undefined {
  const raw = process.env[name];
  if (typeof raw !== 'string') return undefined;
  const trimmed = raw.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

function readBoolEnv(name: string): boolean {
  const raw = readEnv(name);
  return raw === '1' || raw === 'true' || raw === 'TRUE' || raw === 'yes' || raw === 'YES';
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
