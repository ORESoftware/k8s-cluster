type Logger = Pick<Console, 'info' | 'warn'>;

type WebSocketLike = {
  readyState: number;
  send(data: string): void;
  close(): void;
  addEventListener(event: 'open', handler: () => void): void;
  addEventListener(event: 'close', handler: () => void): void;
  addEventListener(event: 'error', handler: (event: unknown) => void): void;
};

type WebSocketConstructor = {
  readonly CONNECTING: number;
  readonly OPEN: number;
  new (url: string): WebSocketLike;
};

export type WorkerFanoutWebSocketOptions = {
  url: string | null;
  logger?: Logger;
  maxQueueDepth?: number;
  reconnectMs?: number;
};

export class WorkerFanoutWebSocket {
  private ws: WebSocketLike | null = null;
  private connecting = false;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private queue: string[] = [];
  private readonly logger: Logger;
  private readonly maxQueueDepth: number;
  private readonly reconnectMs: number;

  constructor(private readonly options: WorkerFanoutWebSocketOptions) {
    this.logger = options.logger ?? console;
    this.maxQueueDepth = Math.max(1, options.maxQueueDepth ?? 500);
    this.reconnectMs = Math.max(250, options.reconnectMs ?? 1000);
  }

  publish(payload: unknown): void {
    if (!this.options.url) return;
    this.queue.push(JSON.stringify(payload));
    if (this.queue.length > this.maxQueueDepth) {
      this.queue.splice(0, this.queue.length - this.maxQueueDepth);
    }
    this.flush();
  }

  destroy(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.ws?.close();
    this.ws = null;
    this.connecting = false;
    this.queue = [];
  }

  private flush(): void {
    if (!this.options.url) return;
    const WebSocketCtor = getWebSocketConstructor();
    if (!WebSocketCtor) {
      this.logger.warn('worker fanout websocket disabled: global WebSocket is not available');
      this.queue = [];
      return;
    }
    if (!this.ws || this.ws.readyState !== WebSocketCtor.OPEN) {
      this.connect(WebSocketCtor);
      return;
    }
    while (this.queue.length > 0 && this.ws.readyState === WebSocketCtor.OPEN) {
      const next = this.queue.shift();
      if (next) this.ws.send(next);
    }
  }

  private connect(WebSocketCtor: WebSocketConstructor): void {
    if (!this.options.url || this.connecting) return;
    if (this.ws && [WebSocketCtor.CONNECTING, WebSocketCtor.OPEN].includes(this.ws.readyState)) {
      return;
    }
    this.connecting = true;
    try {
      const ws = new WebSocketCtor(this.options.url);
      this.ws = ws;
      ws.addEventListener('open', () => {
        this.connecting = false;
        this.logger.info(`worker fanout websocket connected: ${redactWsUrl(this.options.url)}`);
        this.flush();
      });
      ws.addEventListener('close', () => {
        if (this.ws === ws) this.ws = null;
        this.connecting = false;
        this.scheduleReconnect();
      });
      ws.addEventListener('error', (event) => {
        this.logger.warn(`worker fanout websocket error: ${String(event)}`);
      });
    } catch (error) {
      this.connecting = false;
      this.logger.warn(
        `worker fanout websocket connect failed: ${
          error instanceof Error ? error.message : String(error)
        }`,
      );
      this.scheduleReconnect();
    }
  }

  private scheduleReconnect(): void {
    if (!this.options.url || this.reconnectTimer || this.queue.length === 0) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.flush();
    }, this.reconnectMs);
  }
}

export function workerFanoutWsUrlFromEnv(env: NodeJS.ProcessEnv): string | null {
  const explicit = env.WORKER_FANOUT_WS_URL ?? env.GLEAM_WORKER_FANOUT_WS_URL;
  if (explicit?.trim()) return explicit.trim();
  const secret =
    env.WORKER_FANOUT_WS_SECRET ?? env.GLEAM_WORKER_WS_SECRET ?? env.GLEAM_BROADCAST_SECRET;
  if (!secret?.trim()) return null;
  const base =
    env.WORKER_FANOUT_WS_BASE_URL ??
    env.GLEAM_WORKER_WS_BASE_URL ??
    'ws://dd-gleamlang-server.default.svc.cluster.local:8081/worker-ws';
  return `${base.replace(/\/+$/, '')}/${encodeURIComponent(secret.trim())}`;
}

function getWebSocketConstructor(): WebSocketConstructor | null {
  const candidate = (globalThis as unknown as { WebSocket?: WebSocketConstructor }).WebSocket;
  return candidate ?? null;
}

function redactWsUrl(url: string | null): string {
  if (!url) return '';
  try {
    const parsed = new URL(url);
    const parts = parsed.pathname.split('/').filter(Boolean);
    if (parts.length > 1) {
      parts[parts.length - 1] = 'redacted';
      parsed.pathname = `/${parts.join('/')}`;
    }
    return parsed.toString();
  } catch {
    return url.replace(/\/[^/]*$/, '/redacted');
  }
}
