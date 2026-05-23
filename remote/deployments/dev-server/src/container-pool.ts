import { contextFetch } from './wrapped-fetch.js';

export type ContainerPoolClientConfig = {
  baseUrl: string | null;
  authSecret: string | null;
  timeoutMs: number;
};

export type ContainerPoolDispatchRequest = {
  requestId?: string;
  poolId?: string;
  poolSlug?: string;
  path?: string;
  headers?: Record<string, string>;
  payload?: unknown;
  body?: unknown;
};

export type ContainerPoolDispatchResponse = {
  ok: boolean;
  requestId: string;
  poolId?: string;
  poolSlug?: string;
  containerName?: string;
  containerPort?: number;
  targetUrl?: string;
  status?: number;
  body?: unknown;
  elapsedMs?: number;
  error?: string;
};

export function containerPoolConfigFromEnv(env: NodeJS.ProcessEnv): ContainerPoolClientConfig {
  return {
    baseUrl:
      env.CONTAINER_POOL_BASE_URL ??
      env.CONTAINER_POOL_URL ??
      'http://dd-container-pool.default.svc.cluster.local:8102',
    authSecret: env.CONTAINER_POOL_AUTH_SECRET ?? env.SERVER_AUTH_SECRET ?? null,
    timeoutMs: Number(env.CONTAINER_POOL_DISPATCH_TIMEOUT_MS ?? 60_000),
  };
}

export function containerPoolConfigured(config: ContainerPoolClientConfig): boolean {
  return Boolean(config.baseUrl && config.authSecret);
}

export async function dispatchContainerPool(
  config: ContainerPoolClientConfig,
  pool: string,
  request: ContainerPoolDispatchRequest,
): Promise<ContainerPoolDispatchResponse> {
  if (!config.baseUrl) {
    throw new Error('CONTAINER_POOL_BASE_URL is not configured');
  }
  if (!config.authSecret) {
    throw new Error('CONTAINER_POOL_AUTH_SECRET or SERVER_AUTH_SECRET is not configured');
  }

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), Math.max(1_000, config.timeoutMs));
  try {
    const response = await contextFetch(
      `${config.baseUrl.replace(/\/+$/, '')}/pools/${encodeURIComponent(pool)}/dispatch`,
      {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          'x-server-auth': config.authSecret,
        },
        body: JSON.stringify(request),
        signal: controller.signal,
      },
    );
    const text = await response.text();
    let body: unknown = text;
    try {
      body = JSON.parse(text);
    } catch {
      // Keep plain-text error bodies intact.
    }
    if (!response.ok) {
      const message =
        body && typeof body === 'object' && 'error' in body
          ? String((body as { error?: unknown }).error)
          : `container pool dispatch failed with HTTP ${response.status}`;
      throw new Error(message);
    }
    return body as ContainerPoolDispatchResponse;
  } finally {
    clearTimeout(timer);
  }
}
