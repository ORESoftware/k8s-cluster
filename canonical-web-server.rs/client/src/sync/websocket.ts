import type { SyncSocketMessage } from "./types";

export interface SyncWebSocketOptions {
  path?: string;
  onInvalidate: () => void;
  onAccessRevoked?: () => void;
  webSocketFactory?: (url: string) => WebSocket;
  random?: () => number;
}

function socketUrl(path: string): string {
  const url = new URL(path, globalThis.location.href);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  return url.toString();
}

function isSocketMessage(value: unknown): value is SyncSocketMessage {
  return typeof value === "object" && value !== null && "type" in value && typeof value.type === "string";
}

export class SyncInvalidationSocket {
  private readonly options: SyncWebSocketOptions;
  private socket: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectAttempt = 0;
  private stopped = true;

  constructor(options: SyncWebSocketOptions) {
    this.options = options;
  }

  start(): void {
    if (!this.stopped) {
      return;
    }
    this.stopped = false;
    this.connect();
  }

  stop(): void {
    this.stopped = true;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.socket?.close(1000, "client stopped");
    this.socket = null;
  }

  private connect(): void {
    if (this.stopped || typeof globalThis.location === "undefined" || typeof WebSocket === "undefined") {
      return;
    }
    const createSocket = this.options.webSocketFactory ?? ((url: string) => new WebSocket(url));
    const socket = createSocket(socketUrl(this.options.path ?? "/api/v1/sync/ws"));
    this.socket = socket;
    socket.addEventListener("open", () => {
      this.reconnectAttempt = 0;
    });
    socket.addEventListener("message", (event) => {
      try {
        const message: unknown = JSON.parse(String(event.data));
        if (!isSocketMessage(message)) {
          return;
        }
        if (message.type === "sync.invalidated" || message.type === "resync_required") {
          this.options.onInvalidate();
        } else if (message.type === "access_revoked") {
          this.options.onAccessRevoked?.();
          this.stop();
        }
      } catch {
        // Invalidations are hints; malformed frames do not affect cursor correctness.
      }
    });
    socket.addEventListener("close", (event) => {
      if (this.socket === socket) {
        this.socket = null;
      }
      if (event.code === 1008) {
        this.stopped = true;
        this.options.onAccessRevoked?.();
        return;
      }
      this.scheduleReconnect();
    });
    socket.addEventListener("error", () => {
      socket.close();
    });
  }

  private scheduleReconnect(): void {
    if (this.stopped || this.reconnectTimer !== null) {
      return;
    }
    const ceiling = Math.min(1_000 * 2 ** Math.min(this.reconnectAttempt, 8), 60_000);
    this.reconnectAttempt += 1;
    const delay = Math.floor((this.options.random ?? Math.random)() * ceiling);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, delay);
  }
}
