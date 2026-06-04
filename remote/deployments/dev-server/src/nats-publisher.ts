import net from 'node:net';

type NatsUrl = {
  host: string;
  port: number;
};

type PublishInput = {
  subject: string;
  payload: unknown;
};

const MAX_QUEUE_DEPTH = 500;

export class NatsPublisher {
  private socket: net.Socket | null = null;
  private connecting = false;
  private connected = false;
  private reconnectTimer: NodeJS.Timeout | null = null;
  private queue: PublishInput[] = [];

  constructor(
    private readonly url: string | null,
    private readonly logger: Pick<Console, 'warn'> = console,
  ) {}

  publish(subject: string, payload: unknown): void {
    if (!this.url) return;
    this.queue.push({ subject, payload });
    if (this.queue.length > MAX_QUEUE_DEPTH) {
      this.queue.splice(0, this.queue.length - MAX_QUEUE_DEPTH);
    }
    this.flush();
  }

  destroy(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.socket?.destroy();
    this.socket = null;
    this.connected = false;
    this.connecting = false;
  }

  private flush(): void {
    if (!this.url) return;
    if (!this.connected || !this.socket || this.socket.destroyed) {
      this.connect();
      return;
    }

    while (this.queue.length > 0) {
      const next = this.queue.shift();
      if (!next) return;
      const payload = JSON.stringify(next.payload);
      const bytes = Buffer.byteLength(payload);
      this.socket.write(`PUB ${next.subject} ${bytes}\r\n${payload}\r\n`);
    }
  }

  private connect(): void {
    if (this.connecting || this.connected || !this.url) return;
    const parsed = parseNatsUrl(this.url);
    if (!parsed) {
      this.logger.warn(`[nats-publisher] invalid NATS_URL: ${this.url}`);
      return;
    }

    this.connecting = true;
    const socket = net.createConnection(parsed, () => {
      this.connecting = false;
      this.connected = true;
      socket.write('CONNECT {"verbose":false,"pedantic":false,"lang":"node","version":"dd-dev-server"}\r\n');
      this.flush();
    });
    this.socket = socket;

    socket.on('data', (chunk) => {
      const text = chunk.toString('utf8');
      if (text.includes('PING')) {
        socket.write('PONG\r\n');
      }
      if (text.includes('-ERR')) {
        this.logger.warn(`[nats-publisher] server error: ${text.trim()}`);
      }
    });
    socket.on('error', (error) => {
      this.logger.warn(`[nats-publisher] ${error.message}`);
    });
    socket.on('close', () => {
      this.connected = false;
      this.connecting = false;
      if (this.socket === socket) {
        this.socket = null;
      }
      this.scheduleReconnect();
    });
  }

  private scheduleReconnect(): void {
    if (!this.url || this.reconnectTimer || this.queue.length === 0) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.flush();
    }, 1000);
  }
}

function parseNatsUrl(raw: string): NatsUrl | null {
  try {
    const url = new URL(raw);
    if (url.protocol !== 'nats:') return null;
    return {
      host: url.hostname,
      port: url.port ? Number(url.port) : 4222,
    };
  } catch {
    return null;
  }
}
